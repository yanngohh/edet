//! Durable storage: a write-ahead log of decided blocks plus periodic state
//! snapshots. Recovery = load the latest snapshot, replay the WAL tail.
//! Closed history prunes with each snapshot: the reputation quantities are
//! carried state, so blocks below the snapshot height can archive.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::block::{sha256, Block};
use edet_state::types::{Key, MemberId};

/// One entry of the validator-set-change journal — the validator set
/// live as of `height`. A type alias purely to keep signatures readable
/// (clippy's `type_complexity`); no semantic weight beyond the tuple.
pub type ValidatorSetAt = (u64, BTreeMap<MemberId, u64>);

/// One entry of the validator-KEY journal — mirrors `ValidatorSetAt`
/// exactly (same `(height, map)` shape) but carries each validator's signing
/// key instead of its voting power. Kept as a genuinely separate journal
/// (`keys.bin`, its own file) rather than widening `ValidatorSetAt`'s tuple
/// or `validators.bin`'s payload: `validators.bin` is an on-disk format a
/// data dir written by code already has, and this fix must not
/// require that dir to be migrated or fail to open — additive-only, exactly
/// how `validators.bin`/`certificates.bin` alongside the WAL.
pub type ValidatorKeysAt = (u64, BTreeMap<MemberId, Key>);

#[derive(Debug)]
pub enum StoreError {
    Io(std::io::Error),
    Corrupt(String),
    Codec(crate::block::CodecError),
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Io(e)
    }
}
impl From<crate::block::CodecError> for StoreError {
    fn from(e: crate::block::CodecError) -> Self {
        StoreError::Codec(e)
    }
}
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(e) => write!(f, "io: {e}"),
            StoreError::Corrupt(m) => write!(f, "corrupt store: {m}"),
            StoreError::Codec(e) => write!(f, "codec: {e}"),
        }
    }
}
impl std::error::Error for StoreError {}

pub struct Store {
    dir: PathBuf,
    wal: File,
    /// Height -> byte offset of the FRAME START in `wal.bin`. Built once
    /// at `open` by walking the WAL, then kept current incrementally by
    /// `append_block`. This is what turns `find_block` from an O(WAL length)
    /// full re-scan (the amplification DoS the adversarial review found —
    /// every peer sync request paid for the whole chain) into one seek plus
    /// one frame read. `BTreeMap`, not `HashMap`: this codebase keeps that
    /// rule even for node-local, non-replicated structures like this one, on
    /// the general principle that a keyed collection here is a `BTreeMap`
    /// unless there's a specific reason otherwise — and it makes a future
    /// range query ("first height >= N") cheap if ever needed.
    block_index: BTreeMap<u64, u64>,
    /// Same idea as `block_index`, over `certificates.bin`.
    cert_index: BTreeMap<u64, u64>,
    /// Frame starts in `wal.bin` that were found UNUSABLE at `open` — a
    /// failed digest (bit rot) or a payload that would not decode — AND that
    /// `open` proved are not needed, because the snapshot on disk already
    /// covers their height. They are absent from `block_index`, so nothing
    /// can serve them (`find_block`), and `read_wal` skips exactly these
    /// offsets and no others: a frame that rots AFTER this set was computed
    /// still refuses the read, because it was never proved unnecessary.
    degraded: std::collections::BTreeSet<u64>,
}

/// WAL frame: [len: u32 LE][sha256 of payload: 32][payload]. Shared by
/// `wal.bin`, `certificates.bin`, `validators.bin`, `keys.bin` and (
/// written directly against a directory rather than through a `Store`
/// handle — see `write_undecided`) `undecided.bin`.
impl Store {
    pub fn open(dir: impl AsRef<Path>) -> Result<Store, StoreError> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        let mut wal = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(dir.join("wal.bin"))?;

        // Build the height->offset index by walking the WAL once, here,
        // at open. This decodes every block — exactly the cost `Replica::open`
        // is about to pay anyway, right after this returns, to replay the WAL
        // into state. So this doesn't add a new order of magnitude to
        // startup; it just keeps the height each frame decoded to instead of
        // throwing it away. Tolerates a torn tail exactly like the old
        // `read_wal` full-scan did: a partial final frame from a crash
        // mid-append is silently dropped from the index, not an error.
        let mut bytes = Vec::new();
        wal.seek(SeekFrom::Start(0))?;
        wal.read_to_end(&mut bytes)?;
        let snapshot_at = snapshot_height(&dir).unwrap_or(0);
        let (block_index, degraded) = match index_wal(&bytes, snapshot_at) {
            Ok(indexed) => indexed,
            Err(WalDamage::Unrecoverable { at }) => {
                // Roll back to the last good frame rather than refusing to
                // start, and keep everything: `quarantine_tail` copies the
                // whole WAL aside before truncating, so nothing is destroyed
                // and the damage stays available to look at.
                bytes = quarantine_tail(&dir, &mut wal, &bytes, at)?;
                // The prefix is intact by construction — `index_wal` stops at
                // the FIRST frame it cannot recover, so everything before `at`
                // already indexed cleanly. A second failure here would mean
                // the walk is not deterministic, which is a bug rather than
                // bit rot, and it is right to refuse then.
                index_wal(&bytes, snapshot_at).map_err(|d| StoreError::Corrupt(d.to_string()))?
            }
        };
        for offset in &degraded {
            eprintln!(
                "store {}: the WAL frame at offset {offset} is unreadable (bit rot) — it is below the snapshot \
                 height, so nothing needs it: dropped from the index and never served to a peer",
                dir.display()
            );
        }
        let cert_index = Store::build_cert_index(&dir)?;

        Ok(Store { dir, wal, block_index, cert_index, degraded })
    }

    /// **Drop every block and certificate below `keep_from`, compacting both
    /// files in place.**
    ///
    /// A WAL that is never pruned grows with the chain's age, for ever, and an
    /// empty block is paced at one a second — about 31.5 million frames a year
    /// on an idle chain, every one of them read and decoded at every start.
    /// Nothing replays a height below the last snapshot, so nothing needs one.
    ///
    /// **What is lost is the ability to SERVE those heights to a peer**, which
    /// is why the caller keeps a margin below the snapshot and why
    /// `min_block_height` exists: a node that has pruned must say so, or it
    /// answers a sync request for a height it does not have. A chain whose
    /// every node has pruned the same range has those blocks only in whatever
    /// archive its operators kept — which is a property of a log that is
    /// replayable rather than authoritative, and the same one `quarantine_tail`
    /// already relies on.
    ///
    /// Written to a temporary file and renamed, so a crash mid-compaction
    /// leaves the original intact: the frames are the node's only copy of
    /// anything it has not snapshotted.
    fn compact(
        dir: &Path,
        file: &mut File,
        name: &str,
        keep_from: u64,
        height_of: impl Fn(&[u8]) -> Option<u64>,
    ) -> Result<BTreeMap<u64, u64>, StoreError> {
        let mut bytes = Vec::new();
        file.seek(SeekFrom::Start(0))?;
        file.read_to_end(&mut bytes)?;

        let mut kept = Vec::new();
        for (at, payload) in walk_frames_raw(&bytes) {
            let end = walk_end(&bytes, at);
            // An unreadable frame is kept: it is `open`'s business to decide
            // whether it is droppable, and dropping one here would decide it
            // silently on a different rule.
            let keep = match payload.and_then(&height_of) {
                Some(h) => h >= keep_from,
                None => true,
            };
            if keep {
                kept.extend_from_slice(&bytes[at as usize..end]);
            }
        }
        if kept.len() == bytes.len() {
            return Err(StoreError::Corrupt(String::new()));
        }

        let tmp = dir.join(format!("{name}.compact"));
        {
            let mut f = File::create(&tmp)?;
            f.write_all(&kept)?;
            f.sync_data()?;
        }
        fs::rename(&tmp, dir.join(name))?;
        sync_dir(dir)?;
        *file = OpenOptions::new().create(true).append(true).read(true).open(dir.join(name))?;

        let mut index = BTreeMap::new();
        for (at, payload) in walk_frames_raw(&kept) {
            if let Some(h) = payload.and_then(&height_of) {
                index.insert(h, at);
            }
        }
        Ok(index)
    }

    /// Prune both logs below `keep_from`. A no-op when there is nothing below
    /// it, which is the ordinary case on every block but the ones that follow
    /// a snapshot.
    pub fn prune_below(&mut self, keep_from: u64) -> Result<(), StoreError> {
        if keep_from == 0 || self.block_index.keys().next().is_none_or(|&lowest| lowest >= keep_from) {
            return Ok(());
        }
        let dir = self.dir.clone();
        match Store::compact(&dir, &mut self.wal, "wal.bin", keep_from, |p| Block::decode(p).ok().map(|b| b.height)) {
            Ok(index) => {
                self.block_index = index;
                // Every offset the degraded set names has moved, and the frames
                // it named were below a snapshot by construction — so they are
                // gone with everything else below `keep_from`, and a set of
                // stale offsets would skip live frames.
                self.degraded.clear();
            }
            Err(StoreError::Corrupt(e)) if e.is_empty() => return Ok(()),
            Err(e) => return Err(e),
        }
        match Store::compact(&dir, &mut self.certs_file()?, "certificates.bin", keep_from, |p| {
            p.get(..8).map(|h| u64::from_le_bytes(h.try_into().expect("8-byte slice")))
        }) {
            Ok(index) => self.cert_index = index,
            Err(StoreError::Corrupt(e)) if e.is_empty() => {}
            Err(e) => return Err(e),
        }
        Ok(())
    }

    /// The lowest block height this store can still serve. `None` when it
    /// holds no block at all.
    pub fn min_block_height(&self) -> Option<u64> {
        self.block_index.keys().next().copied()
    }

    /// The highest height still in the WAL, or `None` for an empty log.
    ///
    /// What `import-snapshot` asks before it writes: a home already at or past
    /// the imported height would be REWOUND by the import, which is the one
    /// thing a recovery must not do.
    pub fn max_block_height(&self) -> Option<u64> {
        self.block_index.keys().next_back().copied()
    }

    /// The certificate log, opened on demand: unlike the WAL it is not held
    /// open across the store's life, and `compact` needs a handle to replace.
    fn certs_file(&self) -> Result<File, StoreError> {
        Ok(OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(self.dir.join("certificates.bin"))?)
    }

    /// Same walk as the WAL index above, over `certificates.bin`. A
    /// missing file (an older data dir, or a fresh one with no certificate
    /// ever appended) indexes as empty, not an error — matching
    /// `read_certificates`'s existing tolerance for a missing file.
    ///
    /// An unreadable certificate frame is dropped from the index rather
    /// than refusing the whole store — unconditionally, unlike the WAL, and
    /// for a reason that does not depend on any snapshot height: a
    /// certificate is provenance served to peers (`decided_value_at`), never
    /// input to this node's own state. Nothing reconstructs anything from
    /// this file (`read_certificates` has no production caller), so a rotted
    /// entry costs one height's worth of proof this node can hand a syncing
    /// peer — which then asks another node — and refusing to boot over it
    /// would trade a whole validator for that. Dropped from the index means
    /// `find_certificate` answers `None`, so a corrupt certificate is never
    /// served either.
    fn build_cert_index(dir: &Path) -> Result<BTreeMap<u64, u64>, StoreError> {
        let path = dir.join("certificates.bin");
        let mut index = BTreeMap::new();
        if !path.exists() {
            return Ok(index);
        }
        let mut bytes = Vec::new();
        File::open(&path)?.read_to_end(&mut bytes)?;
        for (offset, payload) in walk_frames_raw(&bytes) {
            let decoded = payload.and_then(|p| edet_state::codec::decode::<(u64, Vec<u8>)>(p).ok());
            match decoded {
                // First frame per height wins, matching `block_index`'s rule
                // (see `index_wal`) so the two files answer consistently.
                Some((height, _bytes)) => {
                    index.entry(height).or_insert(offset);
                }
                None => eprintln!(
                    "store {}: the certificate frame at offset {offset} is unreadable — dropped from the index; \
                     this node can no longer serve that height's commit certificate to a syncing peer",
                    dir.display()
                ),
            }
        }
        Ok(index)
    }

    pub fn append_block(&mut self, block: &Block) -> Result<(), StoreError> {
        let payload = block.encode()?;
        let digest = sha256(&payload);
        let len = payload.len() as u32;
        // Capture the frame start BEFORE writing, from a metadata call
        // rather than the file's cursor. The WAL is opened in append mode,
        // so every write lands at EOF regardless of the cursor; querying the
        // length explicitly is the only way to learn where that EOF is
        // without assuming append-mode cursor semantics we don't control.
        let offset = self.wal.metadata()?.len();
        self.wal.write_all(&len.to_le_bytes())?;
        self.wal.write_all(&digest)?;
        self.wal.write_all(&payload)?;
        self.wal.sync_data()?;
        // FIRST frame per height wins, never the last. `Replica::open`'s
        // replay applies the first frame it meets for a height and skips
        // every later one (`if block.height <= height { continue }`), so an
        // index that let a duplicate overwrite the entry would answer
        // `find_block` with a block this node never applied — serving a peer
        // one history while holding another. Unreachable through the commit
        // path (`commit_block_unchecked` refuses anything but `height + 1`),
        // which is exactly why the two rules must be written down as one
        // rather than left to coincide.
        self.block_index.entry(block.height).or_insert(offset);
        Ok(())
    }

    /// Read every usable frame; a torn tail (partial final frame from a crash
    /// mid-append) is tolerated and ignored. Still a full O(WAL length)
    /// scan — used for full replay (`Replica::open`), where every block is
    /// needed anyway, unlike `find_block`'s single-height lookup.
    ///
    /// Skips exactly the frames `open` recorded as `degraded` —
    /// unreadable AND provably below the snapshot height, so replay would
    /// have skipped them regardless. Any OTHER unreadable frame still
    /// refuses the read: it may be a height this node has to replay, and a
    /// silently shortened history would resume the ledger from a state no
    /// other node holds.
    pub fn read_wal(&mut self) -> Result<Vec<Block>, StoreError> {
        self.wal.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        self.wal.read_to_end(&mut bytes)?;
        let mut blocks = Vec::new();
        for (offset, payload) in walk_frames_raw(&bytes) {
            match payload.map(Block::decode) {
                Some(Ok(block)) => blocks.push(block),
                _ if self.degraded.contains(&offset) => continue,
                Some(Err(e)) => return Err(e.into()),
                None => return Err(StoreError::Corrupt(format!("frame at offset {offset}"))),
            }
        }
        Ok(blocks)
    }

    pub fn write_snapshot(&mut self, height: u64, state: &edet_state::State) -> Result<(), StoreError> {
        let payload =
            edet_state::codec::encode(state).map_err(|e| StoreError::Corrupt(format!("snapshot encode: {e}")))?;
        let digest = sha256(&payload);
        let tmp = self.dir.join("snapshot.tmp");
        {
            let mut f = File::create(&tmp)?;
            f.write_all(&height.to_le_bytes())?;
            f.write_all(&digest)?;
            f.write_all(&payload)?;
            f.sync_data()?;
        }
        fs::rename(&tmp, self.dir.join("snapshot.bin"))?;
        sync_dir(&self.dir)?;
        Ok(())
    }

    pub fn read_snapshot(&self) -> Result<Option<(u64, edet_state::State)>, StoreError> {
        let path = self.dir.join("snapshot.bin");
        if !path.exists() {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        File::open(&path)?.read_to_end(&mut bytes)?;
        if bytes.len() < 40 {
            return Err(StoreError::Corrupt("snapshot header".into()));
        }
        let height = u64::from_le_bytes(
            bytes[0..8]
                .try_into()
                .map_err(|_| StoreError::Corrupt("snapshot height".into()))?,
        );
        let digest: [u8; 32] = bytes[8..40]
            .try_into()
            .map_err(|_| StoreError::Corrupt("snapshot digest".into()))?;
        let payload = &bytes[40..];
        if sha256(payload) != digest {
            return Err(StoreError::Corrupt("snapshot digest mismatch".into()));
        }
        let state =
            edet_state::codec::decode(payload).map_err(|e| StoreError::Corrupt(format!("snapshot decode: {e}")))?;
        Ok(Some((height, state)))
    }

    /// (the paper's §Implementation):
    /// append one entry to the validator-set-change journal — the exact
    /// `state.validators` live as of `height`, written only when a committed
    /// block actually changes it (`Suspend`/`ValidatorPower`/`Exit`, rare
    /// governance-adjacent events), never once per block. This is the
    /// "snapshot-adjacent, not full state re-derivation" retention the spec
    /// asks for: a small side journal a resuming node reads directly,
    /// instead of replaying the whole block history just to answer "what
    /// was the validator set as of height N". Same frame format as
    /// `append_block` (`[len:u32][sha256:32][payload]`) but its own file —
    /// additive: a data dir from simply has none, which
    /// `read_validator_history` reads as an empty (not corrupt, not
    /// missing-field-error) history.
    pub fn append_validator_set(&self, height: u64, validators: &BTreeMap<MemberId, u64>) -> Result<(), StoreError> {
        let payload = edet_state::codec::encode(&(height, validators))
            .map_err(|e| StoreError::Corrupt(format!("validator-set encode: {e}")))?;
        let digest = sha256(&payload);
        let len = payload.len() as u32;
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("validators.bin"))?;
        f.write_all(&len.to_le_bytes())?;
        f.write_all(&digest)?;
        f.write_all(&payload)?;
        f.sync_data()?;
        Ok(())
    }

    /// Read every `(height, validators)` entry recorded by
    /// `append_validator_set`, in append (ascending height) order. Tolerates
    /// a torn tail exactly like `read_wal`. A missing file — an older data
    /// dir, or a fresh one with no validator-set change yet — reads as an
    /// empty `Vec`, not an error.
    pub fn read_validator_history(&self) -> Result<Vec<ValidatorSetAt>, StoreError> {
        let path = self.dir.join("validators.bin");
        if !path.exists() {
            return Ok(Vec::new());
        }
        let mut bytes = Vec::new();
        File::open(&path)?.read_to_end(&mut bytes)?;
        walk_frames(&bytes)?
            .into_iter()
            .map(|(_, payload)| {
                edet_state::codec::decode(payload)
                    .map_err(|e| StoreError::Corrupt(format!("validator frame decode: {e}")))
            })
            .collect()
    }

    /// See `crates/node/src/replica.rs`'s `commit_block_unchecked` doc
    /// comment for the exact trigger condition): append one entry to the
    /// validator-KEY journal — the FULL signing-key map of every CURRENT
    /// validator, written only when a committed block actually rotates a
    /// key belonging to a validator who was one both before and after that
    /// block (`RotateFinalize`). Same frame format and append-only file
    /// convention as `append_validator_set`, its own file (`keys.bin`): a
    /// data dir written simply has none, which
    /// `read_validator_key_history` reads as an empty history, not an error.
    pub fn append_validator_keys(&self, height: u64, keys: &BTreeMap<MemberId, Key>) -> Result<(), StoreError> {
        let payload = edet_state::codec::encode(&(height, keys))
            .map_err(|e| StoreError::Corrupt(format!("validator-key encode: {e}")))?;
        let digest = sha256(&payload);
        let len = payload.len() as u32;
        let mut f = OpenOptions::new().create(true).append(true).open(self.dir.join("keys.bin"))?;
        f.write_all(&len.to_le_bytes())?;
        f.write_all(&digest)?;
        f.write_all(&payload)?;
        f.sync_data()?;
        Ok(())
    }

    /// Read every `(height, keys)` entry recorded by `append_validator_keys`,
    /// in append (ascending height) order. Tolerates a torn tail exactly
    /// like `read_validator_history`. A missing file — a data dir written
    /// written before the key journal existed, or a fresh one with no key rotation yet — reads as an
    /// empty `Vec`, not an error: this is exactly what lets
    /// `EdetValidatorSet::build` fall back to each validator's CURRENT key
    /// for every height on such a dir, matching this crate's behaviour
    /// unchanged.
    pub fn read_validator_key_history(&self) -> Result<Vec<ValidatorKeysAt>, StoreError> {
        let path = self.dir.join("keys.bin");
        if !path.exists() {
            return Ok(Vec::new());
        }
        let mut bytes = Vec::new();
        File::open(&path)?.read_to_end(&mut bytes)?;
        walk_frames(&bytes)?
            .into_iter()
            .map(|(_, payload)| {
                edet_state::codec::decode(payload)
                    .map_err(|e| StoreError::Corrupt(format!("validator-key frame decode: {e}")))
            })
            .collect()
    }

    /// The decided block at `height`, or `None` if the WAL holds no such
    /// height. One seek plus one frame read, via `block_index` (built at
    /// `open`, kept current by `append_block`) — never a full `read_wal`
    /// re-scan (decode every block in the chain, then throw away everything but
    /// one) on EVERY call. That matters because
    /// this is the handler behind `AppMsg::GetDecidedValue`
    /// (`Replica::decided_value_at`), which peer sync requests drive — an
    /// untrusted remote could impose O(chain length) work on a target with
    /// each request. The digest check on the single frame is still done
    /// (`read_frame_at`): it is the store's only corruption detector, and
    /// skipping it here — of all places, on a path fed by network peers —
    /// would be exactly backwards.
    pub fn find_block(&mut self, height: u64) -> Result<Option<Block>, StoreError> {
        let Some(&offset) = self.block_index.get(&height) else { return Ok(None) };
        let payload = read_frame_at(&mut self.wal, offset)?;
        Ok(Some(Block::decode(&payload)?))
    }

    /// Append one entry to the commit-certificate journal — the opaque,
    /// caller-encoded bytes of the `CommitCertificate` that decided the block
    /// at `height`. Deliberately opaque (`&[u8]`, not a concrete certificate
    /// type): `store.rs` is part of the crate's default (non-`malachite`)
    /// build, so it must never depend on `malachitebft-core-types`; encoding
    /// a real `CommitCertificate<EdetContext>` into these bytes is
    /// `engine_malachite`'s job (`--features malachite` only). Same manual
    /// frame format as `append_block`/`append_validator_set`
    /// (`[len:u32][sha256:32][payload]`), its own file — a data dir from
    /// simply has none, which `read_certificates` reads as empty.
    ///
    /// Still NOT atomic with `append_block` — a crash between the two
    /// writes can leave one of the pair without the other. `Replica::
    /// commit_block_with_certificate` orders these calls certificate-first so
    /// that the only reachable partial state is an ORPHAN certificate (one
    /// with no matching block), which is harmless: nothing indexes
    /// certificates by looking at blocks, and `decided_value_at` requires
    /// `find_block` to succeed before it even looks at `find_certificate`.
    /// Closing this fully would need a combined WAL frame format that writes
    /// both in one fsync — out of scope for this pass; see the doc comment on
    /// `commit_block_with_certificate` for the reasoning in full.
    ///
    /// `&mut self`: writing also updates `cert_index`, so this can no longer
    /// be `&self` the way it would be without the index.
    pub fn append_certificate(&mut self, height: u64, certificate_bytes: &[u8]) -> Result<(), StoreError> {
        let payload = edet_state::codec::encode(&(height, certificate_bytes))
            .map_err(|e| StoreError::Corrupt(format!("certificate encode: {e}")))?;
        let digest = sha256(&payload);
        let len = payload.len() as u32;
        let path = self.dir.join("certificates.bin");
        // Same reasoning as `append_block` — the frame start is the
        // current file length, read from the filesystem, not assumed from a
        // cursor. This file is reopened fresh on every call (unlike the WAL,
        // there's no long-lived handle on `Store` for it), so there is no
        // cursor to (mis)trust anyway.
        let offset = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
        f.write_all(&len.to_le_bytes())?;
        f.write_all(&digest)?;
        f.write_all(&payload)?;
        f.sync_data()?;
        self.cert_index.insert(height, offset);
        Ok(())
    }

    /// Read every `(height, certificate_bytes)` entry recorded by
    /// `append_certificate`, in append (ascending height) order. Tolerates a
    /// torn tail exactly like `read_wal`/`read_validator_history`. A missing
    /// file reads as an empty `Vec`, not an error.
    pub fn read_certificates(&self) -> Result<Vec<(u64, Vec<u8>)>, StoreError> {
        let path = self.dir.join("certificates.bin");
        if !path.exists() {
            return Ok(Vec::new());
        }
        let mut bytes = Vec::new();
        File::open(&path)?.read_to_end(&mut bytes)?;
        walk_frames(&bytes)?
            .into_iter()
            .map(|(_, payload)| {
                edet_state::codec::decode(payload)
                    .map_err(|e| StoreError::Corrupt(format!("certificate frame decode: {e}")))
            })
            .collect()
    }

    /// The commit-certificate bytes recorded for `height`, or `None` if
    /// none were ever appended for it — same fix as `find_block`, via
    /// `cert_index`: one seek, one frame, digest verified. Opens
    /// `certificates.bin` fresh rather than keeping a long-lived handle on
    /// `Store` (matching `append_certificate`): certificate lookups are not
    /// on the hot WAL-replay path, so there's no reason to hold a second fd
    /// open for the store's whole lifetime.
    pub fn find_certificate(&self, height: u64) -> Result<Option<Vec<u8>>, StoreError> {
        let Some(&offset) = self.cert_index.get(&height) else { return Ok(None) };
        let mut f = File::open(self.dir.join("certificates.bin"))?;
        let payload = read_frame_at(&mut f, offset)?;
        let (_height, certificate_bytes): (u64, Vec<u8>) = edet_state::codec::decode(&payload)
            .map_err(|e| StoreError::Corrupt(format!("certificate frame decode: {e}")))?;
        Ok(Some(certificate_bytes))
    }
}

/// (`engine_malachite.rs`'s `run` — the `AppMsg::StartedRound`/`Decided`
/// handlers): the undecided-proposal cache — every block this node currently
/// holds in its in-memory `pending` map for the CURRENT height, written to
/// `undecided.bin` so a crash-and-restart does not lose a value this node
/// already voted for and force it to fall back to a full sync (a liveness
/// gap, not a safety one: Malachite's own WAL already covers vote
/// persistence, so nothing here is ever load-bearing for double-signing —
/// see `engine_malachite.rs`'s module doc for the measured claim this must
/// not overstate).
///
/// Same manual frame format as `append_block`/`append_validator_set`
/// (`[len:u32][sha256:32][payload]`) but its own file — additive, exactly
/// like `validators.bin`/`keys.bin`: a data dir from simply has
/// none. Unlike those two, though, this is NOT an append-only history: there
/// is exactly one live height's worth of undecided blocks at a time, so each
/// call TRUNCATES and rewrites the whole file rather than appending a new
/// frame — the previous content (a stale, now-superseded snapshot, or the
/// previous height's leftovers) is exactly what should stop existing.
///
/// Free functions taking `dir` directly, not `Store` methods: this file is
/// read once at `run` startup — before the loop, so before `run` has any
/// other reason to hold a `Store` handle open on this directory — and
/// written on every `pending` insertion. Requiring a live `Store` (which
/// re-scans the whole WAL at `open`) just to reach one small side file would
/// make every insertion pay for a WAL scan it does not need.
pub fn write_undecided(dir: impl AsRef<Path>, height: u64, blocks: &[Block]) -> Result<(), StoreError> {
    let payload = edet_state::codec::encode(&(height, blocks))
        .map_err(|e| StoreError::Corrupt(format!("undecided encode: {e}")))?;
    let digest = sha256(&payload);
    let len = payload.len() as u32;
    let mut f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(dir.as_ref().join("undecided.bin"))?;
    f.write_all(&len.to_le_bytes())?;
    f.write_all(&digest)?;
    f.write_all(&payload)?;
    f.sync_data()?;
    Ok(())
}

/// Read back whatever `write_undecided` last wrote — `(height, blocks)`.
/// Deliberately `Option`, never `Result`: a missing file (no data dir, or
/// one written before this cache existed), a torn frame (a crash mid-`write_undecided`, since
/// the truncate-then-write above is NOT atomic the way `write_snapshot`'s
/// tmp-file rename is — a torn tail here is expected, not exceptional), or a
/// frame that fails its digest check all collapse to `None` exactly like
/// `walk_frames`' existing torn-tail tolerance, reusing that same function
/// rather than re-implementing its bounds/digest checks. This is what the
/// caller's "tolerate a missing or torn file as empty" contract requires: an
/// undecided-proposal cache that fails to load must degrade to the node's
/// behaviour (empty `pending`, fall back to sync), never turn into
/// a startup error.
pub fn read_undecided(dir: impl AsRef<Path>) -> Option<(u64, Vec<Block>)> {
    let path = dir.as_ref().join("undecided.bin");
    let mut bytes = Vec::new();
    File::open(&path).ok()?.read_to_end(&mut bytes).ok()?;
    let (_offset, payload) = walk_frames(&bytes).ok()?.into_iter().next()?;
    edet_state::codec::decode::<(u64, Vec<Block>)>(payload).ok()
}

/// Walk the manual `[len:u32 LE][sha256:32][payload]` frame format shared by
/// `wal.bin`/`certificates.bin`/`validators.bin`, verifying each payload's
/// digest as it goes. Returns `(offset, payload)` pairs where `offset` is the
/// byte position of the FRAME START (the `len` field), not the payload — this
/// is exactly what `block_index`/`cert_index` store, since a later seek-and-
/// read (`read_frame_at`) needs to read the header too, not just the payload.
/// `payload` is `None` for a frame whose stored digest does not match its
/// bytes: the frame is structurally intact (its length header still says
/// where the next frame begins, so the walk continues past it) but its
/// contents are not what was written.
///
/// A torn tail (a partial final frame left by a crash mid-append) is
/// tolerated and silently dropped — the frame headers built so far give no
/// way to distinguish "more bytes are coming" from "this is genuinely the
/// last complete frame", so treating an incomplete trailing frame as absent
/// rather than corrupt is the only sound choice.
///
/// What to DO about a failed digest is the caller's decision, and it differs
/// per file — see `index_wal` (needed vs. already superseded) and
/// `build_cert_index` (provenance only). No caller may ever treat a `None`
/// payload as data: the digest is this store's only defence against silent
/// bit rot, and a corrupted frame must never be replayed or served.
/// Make a rename durable. `fs::rename` is atomic, but the directory entry it
/// writes reaches disk only when the directory itself is synced, so a crash
/// right after the rename can come back holding the old name.
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    File::open(dir)?.sync_all()
}

/// Where the frame starting at `at` ends, for a caller that copies frames
/// verbatim rather than decoding them.
fn walk_end(bytes: &[u8], at: u64) -> usize {
    let at = at as usize;
    let len = u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4-byte slice")) as usize;
    at + 36 + len
}

fn walk_frames_raw(bytes: &[u8]) -> Vec<(u64, Option<&[u8]>)> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 36 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4-byte slice")) as usize;
        let start = at + 36;
        if start + len > bytes.len() {
            break; // torn tail
        }
        let digest: [u8; 32] = bytes[at + 4..at + 36].try_into().expect("32-byte slice");
        let payload = &bytes[start..start + len];
        out.push((at as u64, (sha256(payload) == digest).then_some(payload)));
        at = start + len;
    }
    out
}

/// `walk_frames_raw` for the callers that must have every frame or none:
/// `validators.bin` and `keys.bin` (a missing entry silently changes which
/// validator set — and so which quorum — a past height is verified against)
/// and the small single-frame files. A failed digest is an error here, not a
/// gap.
fn walk_frames(bytes: &[u8]) -> Result<Vec<(u64, &[u8])>, StoreError> {
    walk_frames_raw(bytes)
        .into_iter()
        .map(|(at, payload)| {
            payload
                .map(|p| (at, p))
                .ok_or_else(|| StoreError::Corrupt(format!("frame at offset {at}")))
        })
        .collect()
}

/// The height in `snapshot.bin`'s header, read without deserializing the
/// state behind it. Deliberately the SAME (unauthenticated — the frame digest
/// covers only the payload) field `read_snapshot` will hand `Replica::open`
/// moments later, so "this WAL frame is below the snapshot height" always
/// means exactly "the replay that follows will skip it", whatever that field
/// says. `None` for a data dir with no snapshot, which `index_wal` reads as
/// height 0: nothing is superseded yet, so nothing may be degraded.
fn snapshot_height(dir: &Path) -> Option<u64> {
    let mut header = [0u8; 8];
    File::open(dir.join("snapshot.bin")).ok()?.read_exact(&mut header).ok()?;
    Some(u64::from_le_bytes(header))
}

/// Build the WAL's height->offset index, deciding per unreadable frame
/// whether it is archived, or damage the caller must roll back to.
///
/// Refusing the store entirely — permanently — over one flipped bit anywhere
/// in a WAL that is never pruned, and so grows with the chain's age, is wrong
/// in two ways, and they need different answers.
///
/// **A frame the replay would have skipped anyway.** A snapshot already
/// covers its height, so nothing reads it. A node dying over a block nobody
/// was going to read is an availability defect and nothing else; these are
/// dropped from the index and reported (`degraded`).
///
/// **A frame that is still needed.** This is the case that would brick the
/// node for good, and refusing is the wrong half of the right instinct.
/// Refusing to SERVE a corrupt frame is correct and stays. Refusing to START
/// is not: a WAL is a replayable log, not the authority, so the cure is to
/// roll back to the last good frame and let sync bring the rest. The caller
/// does that (`quarantine_tail`), keeping the original file.
///
/// The tolerance is the same one the torn tail already gets, extended by one
/// condition rather than by a separate repair mode: a frame may be dropped
/// only if it is provably not needed. Heights are appended in strictly
/// ascending order (`commit_block_unchecked` accepts nothing but
/// `height + 1`), so the first READABLE frame after an unreadable one bounds
/// the unreadable one's height from above: it is at most `h - 1`. If even
/// that bound is at or below the snapshot height, replay would skip it and
/// dropping it changes nothing. Everything else — including an unreadable
/// frame with no readable frame after it, where there is no bound at all —
/// still refuses the store, because a WAL that quietly lost a height this
/// node has to replay would resume the ledger from a state no other node
/// holds.
///
/// "Unreadable" covers both a failed digest and a payload that will not
/// decode into a `Block`: neither can be indexed, replayed or served, and
/// the caller's question is the same for both.
/// What `index_wal` could not index. Distinguished from `StoreError` because
/// the caller's response is not "fail" but "roll back to here".
enum WalDamage {
    /// The frame at this offset is unreadable and still needed. Everything
    /// from here on is unusable, whether or not it is itself intact.
    Unrecoverable { at: u64 },
}

impl std::fmt::Display for WalDamage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WalDamage::Unrecoverable { at } => write!(f, "unrecoverable WAL frame at offset {at}"),
        }
    }
}

/// Copy the WAL aside, then truncate it at `at`, returning the surviving
/// bytes.
///
/// **Nothing is deleted.** The whole file is copied to
/// `wal.corrupt-at-<offset>.bin` first, so an operator can still examine the
/// damage — and, if the lost heights exist nowhere else, recover them by hand.
/// That matters because the safety of this whole operation rests on the blocks
/// being re-fetchable: on a chain with peers they are, since consensus decided
/// them and any node holding them can serve them back; on a chain with none,
/// they are not, and a store that had silently deleted them would have turned
/// bit rot into data loss.
///
/// Truncation is the right shape rather than a repair, because a WAL is a
/// replayable log and not the authority. Losing the tail costs this node its
/// place in the chain, which sync restores. Losing a frame in the MIDDLE would
/// cost it a height nobody else could tell it about — which is why `index_wal`
/// truncates from the first unrecoverable frame ONWARD rather than skipping it
/// and keeping what follows. A WAL that quietly lost one height would resume
/// the ledger from a state no other node holds, and that is a safety fault
/// where this is only an availability one.
fn quarantine_tail(dir: &Path, wal: &mut File, bytes: &[u8], at: u64) -> Result<Vec<u8>, StoreError> {
    let copy = dir.join(format!("wal.corrupt-at-{at}.bin"));
    fs::write(&copy, bytes)?;
    wal.set_len(at)?;
    wal.sync_all()?;
    eprintln!(
        "store {}: the WAL frame at offset {at} is unreadable and still needed. Rolled the WAL back to that \
         point and kept the original at {} — this node resumes at the last good height and re-syncs the rest \
         from its peers. If this chain has no peers holding those heights, they exist only in that copy.",
        dir.display(),
        copy.display()
    );
    Ok(bytes[..at as usize].to_vec())
}

fn index_wal(
    bytes: &[u8],
    snapshot_height: u64,
) -> Result<(BTreeMap<u64, u64>, std::collections::BTreeSet<u64>), WalDamage> {
    let frames: Vec<(u64, Option<u64>)> = walk_frames_raw(bytes)
        .into_iter()
        .map(|(at, payload)| (at, payload.and_then(|p| Block::decode(p).ok()).map(|b| b.height)))
        .collect();

    let mut degraded = std::collections::BTreeSet::new();
    let mut earliest_damage: Option<u64> = None;
    let mut next_readable_height: Option<u64> = None;
    for (at, height) in frames.iter().rev() {
        match height {
            Some(h) => next_readable_height = Some(*h),
            None => match next_readable_height {
                Some(h) if h.saturating_sub(1) <= snapshot_height => {
                    degraded.insert(*at);
                }
                // Unreadable and needed. The walk runs in REVERSE, so this
                // is not necessarily the first such frame — `earliest_damage`
                // keeps the lowest offset seen, which is where the truncation
                // has to start.
                _ => earliest_damage = Some(earliest_damage.map_or(*at, |e: u64| e.min(*at))),
            },
        }
    }

    if let Some(at) = earliest_damage {
        return Err(WalDamage::Unrecoverable { at });
    }

    // First frame per height wins — the same rule `append_block` follows, and
    // the one `Replica::open`'s replay enforces (a later frame for a height
    // already applied is skipped). See `append_block`.
    let mut index = BTreeMap::new();
    for (at, height) in frames {
        if let Some(h) = height {
            index.entry(h).or_insert(at);
        }
    }
    Ok((index, degraded))
}

/// Seek `file` to `offset` (a frame START, as recorded in `block_index`/
/// `cert_index`) and read exactly that one frame's payload, verifying its
/// digest. The point: `find_block`/`find_certificate` touch the
/// underlying file once per call — one seek, one bounded read — never the
/// whole store.
fn read_frame_at(file: &mut File, offset: u64) -> Result<Vec<u8>, StoreError> {
    // Bound the allocation against the file's actual size BEFORE trusting
    // the on-disk `len` field enough to allocate it: the index that gave us
    // `offset` was built from a digest-verified frame at open/append time,
    // but the header could still bit-rot on disk afterwards, and a header
    // that claims a huge `len` must not turn into a huge allocation attempt
    // on a file that plainly isn't that big.
    let file_len = file.metadata()?.len();
    file.seek(SeekFrom::Start(offset))?;
    let mut header = [0u8; 36];
    file.read_exact(&mut header)?;
    let len = u32::from_le_bytes(header[0..4].try_into().expect("4-byte slice"));
    let digest: [u8; 32] = header[4..36].try_into().expect("32-byte slice");
    if offset + 36 + len as u64 > file_len {
        return Err(StoreError::Corrupt(format!("frame at offset {offset} claims a length past end of file")));
    }
    let mut payload = vec![0u8; len as usize];
    file.read_exact(&mut payload)?;
    if sha256(&payload) != digest {
        return Err(StoreError::Corrupt(format!("frame at offset {offset}")));
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use edet_state::tx::Tx;

    /// A unique-per-test scratch directory under the OS temp dir — no
    /// `tempfile` dependency in this crate, so this mirrors what that crate
    /// would do: a name unique enough that parallel test runs never collide.
    fn scratch_dir(tag: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("edet-store-test-{tag}-{unique}-{:?}", std::thread::current().id()))
    }

    fn sample_block(height: u64) -> Block {
        Block {
            height,
            time_secs: height * 30,
            app_hash: [0x11; 32],
            txs: vec![crate::block::SignedTx {
                tx: Tx::MarkExpired { contract: height },
                nonce: crate::block::counter_nonce(height),
                not_after_epoch: 30,
                signers: vec![],
                signatures: vec![],
            }],
        }
    }

    #[test]
    fn wal_round_trips_and_find_block_locates_by_height() {
        let dir = scratch_dir("wal");
        let mut store = Store::open(&dir).expect("open");
        for h in 1..=3u64 {
            store.append_block(&sample_block(h)).expect("append");
        }
        assert_eq!(store.read_wal().expect("read").len(), 3);
        assert_eq!(store.find_block(2).expect("find").map(|b| b.height), Some(2));
        assert!(store.find_block(99).expect("find").is_none(), "an unwritten height must not be found");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn certificate_journal_round_trips_and_a_missing_dir_reads_as_empty() {
        let dir = scratch_dir("certs");
        let mut store = Store::open(&dir).expect("open");

        // A fresh dir (no certificates ever appended) reads as empty, not an error.
        assert_eq!(store.read_certificates().expect("read empty"), Vec::new());
        assert_eq!(store.find_certificate(1).expect("find on empty"), None);

        store.append_certificate(1, b"cert-for-height-1").expect("append 1");
        store.append_certificate(2, b"cert-for-height-2").expect("append 2");

        let all = store.read_certificates().expect("read");
        assert_eq!(all, vec![(1, b"cert-for-height-1".to_vec()), (2, b"cert-for-height-2".to_vec())]);
        assert_eq!(store.find_certificate(2).expect("find"), Some(b"cert-for-height-2".to_vec()));
        assert_eq!(store.find_certificate(3).expect("find missing"), None);
        fs::remove_dir_all(&dir).ok();
    }

    /// The core claim: `find_block` must locate the right block for every
    /// height across a long chain, and `None` for a height never written —
    /// via the index, not a fall-back full scan. 200 is arbitrary but large
    /// enough that an accidental O(n) full re-parse per lookup (the bug this
    /// is guarding against) would still "work", just slowly — this test only
    /// checks correctness; the point of the fix is the complexity, which
    /// `open_builds_the_index_by_scanning_a_preexisting_data_dir` below
    /// exercises the mechanism of (index built at `open`, not just appended
    /// to incrementally).
    #[test]
    fn find_block_locates_every_height_across_two_hundred_blocks_after_reopen() {
        let dir = scratch_dir("index-200");
        {
            let mut store = Store::open(&dir).expect("open");
            for h in 1..=200u64 {
                store.append_block(&sample_block(h)).expect("append");
            }
        }
        let mut store = Store::open(&dir).expect("reopen");
        for h in 1..=200u64 {
            let found = store
                .find_block(h)
                .expect("find")
                .unwrap_or_else(|| panic!("height {h} must be present"));
            assert_eq!(found.height, h);
        }
        assert!(store.find_block(0).expect("find").is_none(), "height 0 was never written");
        assert!(store.find_block(201).expect("find").is_none(), "an unwritten height must not be found");
        fs::remove_dir_all(&dir).ok();
    }

    /// Proves the `open()`-time scan builds the index, not merely the
    /// incremental `append_block` path — by writing with one `Store` handle,
    /// dropping it (so nothing incremental carries over), then opening a
    /// FRESH `Store` over the same directory and querying immediately with
    /// no further appends.
    #[test]
    fn open_builds_the_index_by_scanning_a_preexisting_data_dir() {
        let dir = scratch_dir("preexisting");
        {
            let mut writer = Store::open(&dir).expect("open to write");
            for h in 1..=5u64 {
                writer.append_block(&sample_block(h)).expect("append");
            }
            writer.append_certificate(3, b"cert-for-3").expect("append certificate");
        } // dropped — a fresh open() must rebuild both indices from disk alone

        let mut reader = Store::open(&dir).expect("reopen a preexisting data dir");
        for h in 1..=5u64 {
            assert_eq!(
                reader.find_block(h).expect("find").map(|b| b.height),
                Some(h),
                "height {h} must be indexed by the open()-time scan alone"
            );
        }
        assert_eq!(reader.find_certificate(3).expect("find cert"), Some(b"cert-for-3".to_vec()));
        fs::remove_dir_all(&dir).ok();
    }

    /// A torn tail (a crash mid-append, leaving a partial final frame)
    /// must not prevent `open()` from succeeding or from indexing every
    /// frame that DID complete — matching `read_wal`'s existing tolerance,
    /// now extended to the index-building scan.
    #[test]
    fn torn_tail_is_tolerated_and_intact_frames_before_it_are_still_indexed() {
        let dir = scratch_dir("torn-tail");
        {
            let mut store = Store::open(&dir).expect("open");
            for h in 1..=5u64 {
                store.append_block(&sample_block(h)).expect("append");
            }
        }
        // Chop bytes off the end of the WAL so the last frame is torn.
        let wal_path = dir.join("wal.bin");
        let full_len = fs::metadata(&wal_path).expect("stat").len();
        let f = OpenOptions::new().write(true).open(&wal_path).expect("open for truncate");
        f.set_len(full_len - 5).expect("truncate mid-frame");
        drop(f);

        let mut store = Store::open(&dir).expect("open must tolerate a torn tail, not error");
        for h in 1..=4u64 {
            assert_eq!(
                store.find_block(h).expect("find").map(|b| b.height),
                Some(h),
                "intact frame at height {h} must still be indexed"
            );
        }
        assert!(store.find_block(5).expect("find torn").is_none(), "the torn final frame must not be indexed");
        fs::remove_dir_all(&dir).ok();
    }

    // --- one rotted archived frame must not refuse the whole store ------

    /// A WAL of `heights`, plus a snapshot at `snapshot_at` if given.
    fn store_with(dir: &Path, heights: std::ops::RangeInclusive<u64>, snapshot_at: Option<u64>) {
        let mut store = Store::open(dir).expect("open");
        for h in heights {
            store.append_block(&sample_block(h)).expect("append");
        }
        if let Some(h) = snapshot_at {
            store.write_snapshot(h, &edet_state::State::default()).expect("write snapshot");
        }
    }

    /// Flip one bit INSIDE the payload of the frame indexed for `height` —
    /// silent bit rot exactly as a failing disk produces it. The frame header
    /// is untouched, so the walk still finds every later frame and only the
    /// digest reveals the damage.
    fn rot_block(dir: &Path, height: u64) {
        let offset = {
            let store = Store::open(dir).expect("open to locate the frame");
            *store
                .block_index
                .get(&height)
                .expect("height must be indexed before it can be rotted")
        };
        let path = dir.join("wal.bin");
        let mut bytes = fs::read(&path).expect("read wal");
        bytes[offset as usize + 36 + 1] ^= 0x01;
        fs::write(&path, bytes).expect("write wal");
    }

    /// The proven defect: one bit flipped in an archived block permanently
    /// refused the store — `Store::open` walked and digest-checked EVERY
    /// frame before anything decided which heights were still needed, so a
    /// `Corrupt` propagated through `Replica::open` → `serve::build` →
    /// `EdetApp::start` and the node never came up again. Replay would have
    /// skipped that block outright: the snapshot already covers its height.
    #[test]
    fn a_rotted_frame_below_the_snapshot_height_degrades_instead_of_refusing_the_store() {
        let dir = scratch_dir("rot-archived");
        store_with(&dir, 1..=6, Some(6));
        rot_block(&dir, 1);

        let mut store = Store::open(&dir).expect("a rotted ARCHIVED frame must not refuse the store");
        assert!(
            store.find_block(1).expect("find").is_none(),
            "a corrupt frame must never be served to a peer — degraded means absent, not readable"
        );
        for h in 2..=6u64 {
            assert_eq!(store.find_block(h).expect("find").map(|b| b.height), Some(h), "height {h} is still intact");
        }
        let replayed: Vec<u64> = store
            .read_wal()
            .expect("replay must not error either")
            .iter()
            .map(|b| b.height)
            .collect();
        assert_eq!(replayed, vec![2, 3, 4, 5, 6], "replay skips exactly the degraded frame, and nothing else");
        fs::remove_dir_all(&dir).ok();
    }

    /// The other half, and the rule that must not regress: a rotted frame the
    /// node still NEEDS ends the usable WAL there. Everything from it onward
    /// is rolled back, whether or not those later frames are themselves
    /// intact — a WAL that quietly kept the heights ABOVE a lost one would
    /// resume the ledger from a state no other node holds, which is a safety
    /// fault where this is only an availability one.
    ///
    /// What changed is what happens next. Refusing to SERVE a corrupt frame
    /// is right and stays; refusing to START was the wrong half of the same
    /// instinct, and it made one flipped bit permanent.
    #[test]
    fn a_rotted_frame_that_is_still_needed_ends_the_wal_there() {
        for (rot_at, snapshot_at) in [(5u64, Some(3u64)), (4, Some(3)), (2, None)] {
            let dir = scratch_dir(&format!("rot-needed-{rot_at}-{}", snapshot_at.unwrap_or(0)));
            store_with(&dir, 1..=6, snapshot_at);
            rot_block(&dir, rot_at);
            let mut store = Store::open(&dir).expect("bit rot must not brick the node");
            for h in 1..rot_at {
                assert!(store.find_block(h).expect("find").is_some(), "height {h} is below the damage and survives");
            }
            for h in rot_at..=6 {
                assert!(
                    store.find_block(h).expect("find").is_none(),
                    "height {h} is at or above the damage and must be rolled back, not kept"
                );
            }
            assert!(
                fs::read_dir(&dir)
                    .expect("list")
                    .filter_map(|e| e.ok())
                    .any(|e| e.file_name().to_string_lossy().starts_with("wal.corrupt-at-")),
                "and the damaged WAL is quarantined rather than deleted"
            );
            fs::remove_dir_all(&dir).ok();
        }
    }

    /// The boundary is exact: a frame AT the snapshot height is already
    /// superseded (replay skips `block.height <= height`), one above it is
    /// not. Same chain, same rot, one height apart.
    #[test]
    fn the_degradable_boundary_is_the_snapshot_height_itself() {
        let tolerated = scratch_dir("rot-boundary-at");
        store_with(&tolerated, 1..=6, Some(3));
        rot_block(&tolerated, 3);
        assert!(Store::open(&tolerated).is_ok(), "a frame at the snapshot height is already superseded");
        fs::remove_dir_all(&tolerated).ok();

        let rolled_back = scratch_dir("rot-boundary-above");
        store_with(&rolled_back, 1..=6, Some(3));
        rot_block(&rolled_back, 4);
        let mut store = Store::open(&rolled_back).expect("still not fatal");
        assert!(store.find_block(3).expect("find").is_some());
        assert!(store.find_block(4).expect("find").is_none(), "one height above it is still needed, so the WAL ends");
        fs::remove_dir_all(&rolled_back).ok();
    }

    /// A rotted frame with nothing readable after it has no upper bound on
    /// its own height, so it cannot be proved unnecessary. It is rolled back
    /// rather than degraded on a guess — the conservative reading is still
    /// the one taken, it just no longer costs the node its life.
    #[test]
    fn a_rotted_final_frame_is_never_degraded_on_a_guess() {
        let dir = scratch_dir("rot-tail");
        store_with(&dir, 1..=6, Some(6));
        rot_block(&dir, 6);
        let mut store = Store::open(&dir).expect("rolled back, not refused");
        assert!(
            store.find_block(6).expect("find").is_none(),
            "with no readable frame after it, nothing bounds this frame's height — it must not be kept on a guess"
        );
        assert!(store.find_block(5).expect("find").is_some(), "and everything below it is untouched");
        fs::remove_dir_all(&dir).ok();
    }

    /// The index and replay must answer the same question the same way. The
    /// index must not keep the LAST frame written for a height while replay
    /// applies the FIRST, so a duplicated height would have made this node
    /// serve a peer a block it never applied. Unreachable through
    /// `commit_block_unchecked` today; written down as one rule so it stays
    /// that way.
    #[test]
    fn the_index_and_replay_agree_about_a_duplicated_height() {
        let dir = scratch_dir("duplicate-height");
        let mut first = sample_block(1);
        first.time_secs = 111;
        let mut second = sample_block(1);
        second.time_secs = 222;
        {
            let mut store = Store::open(&dir).expect("open");
            store.append_block(&first).expect("append first");
            store.append_block(&second).expect("append duplicate");
            assert_eq!(
                store.find_block(1).expect("find").map(|b| b.time_secs),
                Some(111),
                "the incremental index must keep the first frame for a height"
            );
        }
        let mut store = Store::open(&dir).expect("reopen");
        let served = store.find_block(1).expect("find").expect("height 1 present").time_secs;
        let replayed = store.read_wal().expect("replay").first().expect("at least one block").time_secs;
        assert_eq!(served, 111, "the open()-time index must keep the first frame too");
        assert_eq!(served, replayed, "what a peer is served must be what this node applied");
        fs::remove_dir_all(&dir).ok();
    }

    /// A rotted certificate frame costs one height's provenance, never the
    /// store: certificates are served to peers, never replayed into this
    /// node's own state. It must also not be served — a corrupt certificate
    /// proves nothing.
    #[test]
    fn a_rotted_certificate_frame_drops_that_height_and_keeps_the_rest() {
        let dir = scratch_dir("rot-cert");
        {
            let mut store = Store::open(&dir).expect("open");
            store.append_certificate(1, b"cert-for-height-1").expect("append 1");
            store.append_certificate(2, b"cert-for-height-2").expect("append 2");
        }
        let path = dir.join("certificates.bin");
        let mut bytes = fs::read(&path).expect("read certificates");
        bytes[36 + 1] ^= 0x01; // inside the first frame's payload
        fs::write(&path, bytes).expect("write certificates");

        let store = Store::open(&dir).expect("a rotted certificate must not refuse the store");
        assert_eq!(store.find_certificate(1).expect("find"), None, "a corrupt certificate is never served");
        assert_eq!(
            store.find_certificate(2).expect("find"),
            Some(b"cert-for-height-2".to_vec()),
            "the intact certificates are unaffected"
        );
        fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod bitrot_tests {
    use super::*;

    fn block_at(height: u64) -> Block {
        Block { height, time_secs: height * 60, app_hash: [0u8; 32], txs: vec![] }
    }

    fn dir_for(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("edet-bitrot-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// Flip one byte inside the payload of the frame holding `height`.
    fn rot(dir: &Path, offsets: &BTreeMap<u64, u64>, height: u64) {
        let at = offsets[&height] as usize;
        let path = dir.join("wal.bin");
        let mut bytes = fs::read(&path).expect("read wal");
        // Frame layout is `[len:u32][sha256:32][payload]`, so the first
        // payload byte sits 36 in. Corrupting the PAYLOAD (not the header)
        // is what bit rot looks like: the walk still finds the next frame,
        // and only the digest catches it.
        bytes[at + 36] ^= 0xff;
        fs::write(&path, bytes).expect("write wal");
    }

    /// The defect this closes: one flipped bit in a WAL frame that replay
    /// still needs must not refuse the store PERMANENTLY, with no repair path.
    /// Detection without recovery turns a recoverable fault into a terminal
    /// one — and the WAL is a replayable log, not the authority, so the cure
    /// is to roll back to the last good frame and re-sync the rest.
    #[test]
    fn a_rotted_frame_rolls_the_wal_back_instead_of_bricking_the_node() {
        let dir = dir_for("needed");
        let offsets = {
            let mut w = Store::open(&dir).expect("open");
            for h in 1..=5 {
                w.append_block(&block_at(h)).expect("append");
            }
            w.block_index.clone()
        };
        rot(&dir, &offsets, 3);

        let mut reopened = Store::open(&dir).expect("PROVEN: bit rot must not brick the node");
        // Heights before the damage survive; the damaged one and everything
        // after it are gone, to be re-fetched from peers.
        assert!(reopened.find_block(1).expect("read").is_some());
        assert!(reopened.find_block(2).expect("read").is_some());
        for h in 3..=5 {
            assert!(reopened.find_block(h).expect("read").is_none(), "height {h} must be rolled back");
        }
        // And nothing was destroyed: the original is kept for an operator who
        // has no peer to re-fetch from.
        let kept: Vec<_> = fs::read_dir(&dir)
            .expect("list")
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("wal.corrupt-at-"))
            .collect();
        assert_eq!(kept.len(), 1, "the damaged WAL must be quarantined, not deleted");
        assert!(fs::metadata(kept[0].path()).expect("stat").len() > 0);

        // Idempotent: a second open of the now-consistent store is quiet.
        let again = Store::open(&dir).expect("reopen");
        assert_eq!(again.block_index.len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    /// The other half, unchanged: a frame the replay would SKIP anyway,
    /// because a snapshot already covers its height, is dropped from the
    /// index rather than rolled back to. Rolling back there would throw away
    /// good heights above it to no purpose.
    #[test]
    fn a_rotted_frame_below_the_snapshot_is_merely_degraded() {
        let dir = dir_for("superseded");
        let offsets = {
            let mut w = Store::open(&dir).expect("open");
            for h in 1..=5 {
                w.append_block(&block_at(h)).expect("append");
            }
            w.write_snapshot(4, &edet_state::State::default()).expect("snapshot at 4");
            w.block_index.clone()
        };
        rot(&dir, &offsets, 2);

        let mut reopened = Store::open(&dir).expect("a superseded frame must not refuse the store");
        assert!(reopened.find_block(2).expect("read").is_none(), "the rotted frame is dropped");
        assert!(reopened.find_block(5).expect("read").is_some(), "but the heights above it survive");
        assert!(
            !dir.join("wal.corrupt-at-0.bin").exists(),
            "nothing was rolled back, so nothing should have been quarantined"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A torn tail — a partial final frame from a crash mid-append — is still
    /// tolerated silently and must NOT trigger a rollback: there is no damage,
    /// only an incomplete write that was never acknowledged.
    #[test]
    fn a_torn_tail_is_not_treated_as_damage() {
        let dir = dir_for("torn");
        {
            let mut w = Store::open(&dir).expect("open");
            for h in 1..=3 {
                w.append_block(&block_at(h)).expect("append");
            }
        }
        let path = dir.join("wal.bin");
        let mut bytes = fs::read(&path).expect("read");
        bytes.truncate(bytes.len() - 4); // a half-written final frame
        fs::write(&path, bytes).expect("write");

        let mut reopened = Store::open(&dir).expect("a torn tail is not corruption");
        assert!(reopened.find_block(2).expect("read").is_some());
        assert!(reopened.find_block(3).expect("read").is_none(), "the incomplete frame is simply absent");
        assert!(!dir.join("wal.corrupt-at-0.bin").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
