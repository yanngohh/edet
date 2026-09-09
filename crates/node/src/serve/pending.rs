//! The pending-signature pool: the wire mechanism behind multi-party
//! transactions.
//!
//! A bilateral contract (Accept, Sale, Settle, …) or a threshold action
//! (guardian rotation) needs signatures from more than one member. No
//! client ever holds another member's key, so the initiating device signs
//! what it can and parks the transaction here; the counterparties' devices
//! list what awaits them, review, and add their own signatures. When the
//! distinct required signers reach `min_sigs`, the node assembles the
//! `SignedTx` and submits it through the normal verified ingress.
//!
//! Entries are keyed by the transaction digest (the exact bytes everyone
//! signs), co-signs are gossiped like transactions so a pair split across
//! two nodes converges, and completion is idempotent: every node that
//! reaches the threshold submits, and the mempool dedups by content hash.

use std::collections::{BTreeMap, BTreeSet};

use edet_state::types::{Key, MemberId, Party};
use edet_state::{State, Tx};
use serde::{Deserialize, Serialize};

use crate::block::{sha256, tx_digest, SignedTx};

/// TTL for unsigned proposals, in wall-clock seconds (`Node::propose_time`):
/// a proposal nobody finished co-signing in three months is abandoned.
const PENDING_TTL_SECS: u64 = 90 * 86_400;
const MAX_ENTRIES: usize = 1024;
const MAX_TOMBSTONES: usize = 4096;

/// A signature request as it travels over the wire (client → node → peers).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingSignReq {
    pub tx: Tx,
    /// The envelope fields that make this proposal's digest unique and
    /// bound to a validity window, exactly like a directly-submitted
    /// `SignedTx`. Chosen by the INITIATOR and carried by every co-signer's
    /// own request too — a co-signer must reproduce the identical digest to
    /// land in the same pool entry (entries are keyed by digest), so their
    /// client reads these back off the `pending` view rather than choosing
    /// its own.
    pub nonce: [u8; 16],
    pub not_after_epoch: u64,
    /// Parties whose signatures this transaction wants.
    ///
    /// A `Party::Key` here is a counterparty with no account yet — the case
    /// a trade seats. They may co-sign an entry that names them, and may OPEN
    /// one only on a member's invitation (`invite`): the pool's occupancy
    /// bound is per initiator and keys are free, so an initiator has to be
    /// somebody the ledger can count, and an invitation makes the inviting
    /// member that somebody.
    pub required: Vec<Party>,
    /// How many distinct members of `required` must sign (threshold
    /// actions like guardian rotation need fewer than all).
    pub min_sigs: usize,
    pub signer: Key,
    pub signature: Vec<u8>,
    /// A member's standing invitation to be bought from, carried by a key
    /// with no account that opens its own first purchase. Absent on every
    /// other request.
    #[serde(default)]
    pub invite: Option<Invite>,
}

/// **A member's invitation to be bought from**, the thing that pays for a
/// pool entry a key opens.
///
/// A key cannot be charged for occupancy, so an entry it opens is charged
/// to the member it names — and only with that member's signature over
/// this, or anybody who knew a member id could keep that member's inbox
/// full of junk from fresh keys. The seller's wallet mints one when it shows
/// its "pay me" QR and the buyer's wallet carries it back with the purchase.
/// It is bounded three ways: it expires (`not_after_secs`), what it opens is
/// spent from the inviter's own rate bucket, and an inviter holds at most
/// `MAX_INVITED_PER_MEMBER` such entries at once. A captured invitation
/// therefore buys a few junk requests into ONE member's inbox until it
/// expires, each visible and declinable, and nothing global.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Invite {
    /// The inviting member's key: the party whose bucket and inbox this
    /// entry is charged to.
    pub key: Key,
    /// Wall-clock seconds after which the invitation is void.
    pub not_after_secs: u64,
    pub nonce: [u8; 16],
    /// Ed25519 over `invite_message`.
    pub signature: Vec<u8>,
}

/// How many entries opened on one member's invitations may be open at once.
pub const MAX_INVITED_PER_MEMBER: usize = 8;

/// The bytes an invitation signs: a domain tag, the chain it is for, the
/// inviting key, the expiry and the nonce, newline-separated, exactly like
/// the viewer credential's message.
///
/// Mirrored client-side by `ui/src/lib/invite.ts::inviteMessage`; both are
/// pinned against the same fixed vector (the Rust test in this module and
/// `ui/src/lib/__tests__/invite.test.ts`), so a change on either side
/// breaks a test on that side.
pub fn invite_message(chain_id: &str, key: &Key, not_after_secs: u64, nonce: &[u8; 16]) -> Vec<u8> {
    let key_hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
    let nonce_hex: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
    format!("edet-invite-v1\n{chain_id}\n{key_hex}\n{not_after_secs}\n{nonce_hex}").into_bytes()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingDeclineReq {
    /// Hex digest of the transaction being declined.
    pub digest: String,
    pub signer: Key,
    pub signature: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct PendingEntry {
    pub tx: Tx,
    pub nonce: [u8; 16],
    pub not_after_epoch: u64,
    pub required: Vec<Party>,
    pub min_sigs: usize,
    /// The member whose occupancy this entry counts against: whoever opened
    /// it, or the inviter when a key did.
    pub initiator: MemberId,
    /// Who actually opened it — a member, or the invited key. What an inbox
    /// shows as "from".
    pub opener: Party,
    pub created_secs: u64,
    /// Verified signatures collected so far (key → signature bytes).
    pub sigs: BTreeMap<Key, Vec<u8>>,
}

/// How many entries one initiator may hold while the pool is full.
///
/// The cap is a FAIR SHARE, not a per-member quota: it binds only once the
/// pool is at `MAX_ENTRIES`, so ordinary use is unaffected and a member with
/// genuinely many proposals in flight is never throttled by a pool that has
/// room. Without it, occupancy was first-come: one member opening
/// `MAX_ENTRIES` self-signed proposals that never complete and never age out
/// (the TTL is 90 days) locked every other member out of multi-party
/// transactions network-wide, since the pool's state is gossiped.
const MAX_ENTRIES_PER_INITIATOR_WHEN_FULL: usize = 16;

#[derive(Default)]
pub struct PendingPool {
    entries: BTreeMap<[u8; 32], PendingEntry>,
    /// Digests that completed or were declined — refuse resurrection by
    /// late gossip.
    ///
    /// Insertion-ordered alongside the set, so the bound evicts the OLDEST
    /// rather than forgetting everything. A `clear()` on overflow meant a
    /// flood of `MAX_TOMBSTONES + 1` throwaway proposals erased the record of
    /// a victim's decline — and their captured co-signature, which the pool
    /// gossips network-wide by design, could then be replayed to assemble the
    /// very transaction they had refused. Its signatures are genuine over
    /// those bytes, so the verified ingress accepts it.
    tombstones: BTreeSet<[u8; 32]>,
    tombstone_order: std::collections::VecDeque<[u8; 32]>,
}

/// Message a decliner signs: bound to the digest but distinct from the
/// transaction's own signing payload, so a decline can never double as a
/// co-sign (or vice versa).
///
/// Mirrored client-side by `ui/src/lib/submit.ts::declineMessage`; the two
/// MUST agree byte-for-byte or a decline signed in the app fails to verify
/// here. Both are pinned against the same fixed vector — the Rust test in
/// this module and the vitest `ui/src/lib/__tests__/decline-message.test.ts`
/// — so a change to the marker bytes breaks a test on whichever side changed.
pub fn decline_message(digest: &[u8; 32]) -> [u8; 32] {
    let mut bytes = Vec::with_capacity(32 + 12);
    bytes.extend_from_slice(digest);
    bytes.extend_from_slice(b"edet-decline");
    sha256(&bytes)
}

fn verify_sig(key: &Key, message: &[u8; 32], signature: &[u8]) -> bool {
    verify_bytes(key, message, signature)
}

fn verify_bytes(key: &Key, message: &[u8], signature: &[u8]) -> bool {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let Ok(vk) = VerifyingKey::from_bytes(key) else { return false };
    let Ok(sig) = Signature::from_slice(signature) else { return false };
    vk.verify(message, &sig).is_ok()
}

/// **Does this request carry a valid invitation, and from whom?** `Some` names
/// the inviting member and their key — the key the door charges.
///
/// An invitation is to BUY from the inviter and nothing else: the opening key
/// must be the debtor, the inviter the creditor, the two of them the whole
/// required set. A junk transaction would still be bounded by the cap, but
/// the inviter's inbox is for purchases, so the door refuses anything else.
/// Called at the door (for the bucket) and in `sign` (for the room), so there
/// is one rule.
pub fn invited_by(state: &State, req: &PendingSignReq, now_secs: u64) -> Option<(MemberId, Key)> {
    let inv = req.invite.as_ref()?;
    if inv.not_after_secs < now_secs {
        return None;
    }
    let inviter = state.member_of_key(&inv.key)?;
    if !verify_bytes(
        &inv.key,
        &invite_message(&state.chain_id, &inv.key, inv.not_after_secs, &inv.nonce),
        &inv.signature,
    ) {
        return None;
    }
    let buyer = Party::Key(req.signer);
    let names_inviter = |p: &Party| *p == Party::Member(inviter) || *p == Party::Key(inv.key);
    let purchase = match &req.tx {
        Tx::Sale { seller, buyer: b, .. } => *b == buyer && names_inviter(seller),
        Tx::Accept { debtor, creditor, .. } => *debtor == buyer && names_inviter(creditor),
        _ => false,
    };
    if !purchase {
        return None;
    }
    if req.required.len() != 2
        || req.min_sigs != 2
        || !req.required.contains(&buyer)
        || !req.required.iter().any(names_inviter)
    {
        return None;
    }
    Some((inviter, inv.key))
}

/// The party whose verified signature this request carries, if any.
///
/// Split out so the ingress can establish "this really signed these bytes"
/// BEFORE the request is allowed to occupy a rate-limiter slot — the limiter
/// keys on `signer`, and a key that costs nothing to mint must not be able to
/// claim a bucket. `PendingPool::sign` calls the same function, so there is one
/// rule rather than a cheap check at the door and a real one inside.
///
/// A key that resolves to no member is a `Party::Key` rather than a refusal,
/// because a newcomer signs their own first trade before they have
/// an account. That does NOT weaken the bucket argument, and the door is where
/// the difference is enforced: `PendingPool::awaits` admits such a signer only
/// against an entry a member already opened naming that exact key, so the
/// buckets a stranger can claim are bounded by entries somebody with standing
/// paid the occupancy for.
pub fn verified_signer(state: &State, req: &PendingSignReq, digest: &[u8; 32]) -> Option<Party> {
    if !verify_sig(&req.signer, digest, &req.signature) {
        return None;
    }
    Some(match state.member_of_key(&req.signer) {
        Some(id) => Party::Member(id),
        None => Party::Key(req.signer),
    })
}

/// The digest a co-sign request refers to, or `None` if it does not encode.
pub fn request_digest(state: &State, req: &PendingSignReq) -> Option<[u8; 32]> {
    tx_digest(&state.chain_id, &req.tx, &req.nonce, req.not_after_epoch).ok()
}

/// Which required parties have a verified signature on the entry.
fn signed_parties(state: &State, entry: &PendingEntry) -> BTreeSet<Party> {
    entry
        .sigs
        .keys()
        .map(|k| match state.member_of_key(k) {
            Some(id) => Party::Member(id),
            None => Party::Key(*k),
        })
        .filter(|p| entry.required.contains(p))
        .collect()
}

pub enum SignOutcome {
    /// Signature recorded; `new` says whether it changed the entry (drives
    /// gossip, exactly like mempool acceptance does for transactions).
    Recorded {
        new: bool,
    },
    /// Threshold reached: the assembled transaction to submit. The entry is
    /// tombstoned.
    ///
    /// Boxed because a `Tx` names its trade parties by `Party`,
    /// and a party carries a 32-byte key — so the assembled envelope dwarfs
    /// every other variant here, and every `Recorded` would pay for it.
    Complete(Box<SignedTx>),
    Rejected(&'static str),
}

/// (digest, entry) pairs for a member's pending proposals, borrowed from the
/// pool. Named to keep `for_member`'s return type readable.
pub type PendingList<'a> = Vec<([u8; 32], &'a PendingEntry)>;

impl PendingPool {
    /// Insert or co-sign a proposal. Every signature is verified against
    /// the transaction digest and mapped to a required member before it
    /// counts.
    pub fn sign(&mut self, state: &State, req: PendingSignReq, now_secs: u64) -> SignOutcome {
        self.prune(now_secs);
        let Ok(digest) = tx_digest(&state.chain_id, &req.tx, &req.nonce, req.not_after_epoch) else {
            return SignOutcome::Rejected("undigestable transaction");
        };
        if self.tombstones.contains(&digest) {
            return SignOutcome::Rejected("already completed or declined");
        }
        if req.required.is_empty() || req.min_sigs == 0 || req.min_sigs > req.required.len() {
            return SignOutcome::Rejected("bad signer requirements");
        }
        let Some(party) = verified_signer(state, &req, &digest) else {
            return SignOutcome::Rejected("bad signature or unknown signer key");
        };
        if !req.required.contains(&party) {
            return SignOutcome::Rejected("signer is not a required party");
        }
        // Only a member may OPEN an entry, in person or by invitation. A key
        // with no account can co-sign one that names it — the newcomer
        // signing a first trade a member recorded — and can open its own
        // first purchase on a member's invitation, which charges that member:
        // occupancy is accounted per initiator, keys are free, so an initiator
        // has to be somebody the ledger can count.
        let (member, opener) = match (party, self.entries.contains_key(&digest)) {
            (Party::Member(id), _) => (id, Party::Member(id)),
            (Party::Key(_), true) => (self.entries[&digest].initiator, self.entries[&digest].opener),
            (Party::Key(k), false) => match invited_by(state, &req, now_secs) {
                Some((inviter, _)) => {
                    let open = self
                        .entries
                        .values()
                        .filter(|e| e.initiator == inviter && matches!(e.opener, Party::Key(_)))
                        .count();
                    if open >= MAX_INVITED_PER_MEMBER {
                        return SignOutcome::Rejected("too many open invitations for this member");
                    }
                    (inviter, Party::Key(k))
                }
                None => {
                    return SignOutcome::Rejected(
                        "only a member, or a key holding a member's invitation, may open a proposal",
                    )
                }
            },
        };
        if self.entries.len() >= MAX_ENTRIES && !self.entries.contains_key(&digest) {
            // Full: admit only if this initiator is within its fair share, so
            // the member who filled the pool is the one refused rather than
            // everybody else.
            let held = self.entries.values().filter(|e| e.initiator == member).count();
            if held >= MAX_ENTRIES_PER_INITIATOR_WHEN_FULL {
                return SignOutcome::Rejected("pending pool full");
            }
            // Reclaim from whoever is over their share before refusing a
            // member who is inside theirs. Oldest first, so a long-abandoned
            // proposal yields to a live one.
            let hogs: Vec<[u8; 32]> = {
                let mut by_initiator: BTreeMap<MemberId, Vec<([u8; 32], u64)>> = BTreeMap::new();
                for (d, e) in &self.entries {
                    by_initiator.entry(e.initiator).or_default().push((*d, e.created_secs));
                }
                by_initiator
                    .into_values()
                    .filter(|v| v.len() > MAX_ENTRIES_PER_INITIATOR_WHEN_FULL)
                    .flat_map(|mut v| {
                        v.sort_by_key(|&(d, created)| (std::cmp::Reverse(created), d));
                        v.into_iter().take(1).map(|(d, _)| d).collect::<Vec<_>>()
                    })
                    .collect()
            };
            if hogs.is_empty() {
                return SignOutcome::Rejected("pending pool full");
            }
            for d in hogs {
                self.entries.remove(&d);
            }
        }
        let entry = self.entries.entry(digest).or_insert_with(|| PendingEntry {
            tx: req.tx.clone(),
            nonce: req.nonce,
            not_after_epoch: req.not_after_epoch,
            required: req.required.clone(),
            min_sigs: req.min_sigs,
            initiator: member,
            opener,
            created_secs: now_secs,
            sigs: BTreeMap::new(),
        });
        let new = entry.sigs.insert(req.signer, req.signature).is_none();

        if signed_parties(state, entry).len() >= entry.min_sigs {
            let entry = self.entries.remove(&digest).expect("entry present");
            self.tombstone(digest);
            let (signers, signatures): (Vec<Key>, Vec<Vec<u8>>) = entry.sigs.into_iter().unzip();
            return SignOutcome::Complete(Box::new(SignedTx {
                tx: entry.tx,
                nonce: entry.nonce,
                not_after_epoch: entry.not_after_epoch,
                signers,
                signatures,
            }));
        }
        SignOutcome::Recorded { new }
    }

    /// Decline (any required party or the initiator): removes the proposal
    /// and tombstones it against re-gossip. Returns whether anything changed.
    pub fn decline(&mut self, state: &State, digest: [u8; 32], signer: &Key, signature: &[u8]) -> bool {
        if !verify_sig(signer, &decline_message(&digest), signature) {
            return false;
        }
        let Some(entry) = self.entries.get(&digest) else { return false };
        let party = match state.member_of_key(signer) {
            Some(id) => Party::Member(id),
            None => Party::Key(*signer),
        };
        // A newcomer named by key may refuse a trade they were offered, which
        // is the same right every other required party has.
        if !entry.required.contains(&party) && Party::Member(entry.initiator) != party {
            return false;
        }
        self.entries.remove(&digest);
        self.tombstone(digest);
        true
    }

    /// Proposals involving `party`: those still awaiting their signature, and
    /// those they signed or initiated that wait on others.
    ///
    /// Keyed by party rather than by member id, so a device whose
    /// key has no account yet can still see the first trade it was offered.
    /// Without that the newcomer would be invisible to the one mechanism that
    /// collects a second signature, and their first trade could only ever be
    /// assembled on one device.
    pub fn for_party(&self, state: &State, party: Party) -> (PendingList<'_>, PendingList<'_>) {
        let mut awaiting = Vec::new();
        let mut mine = Vec::new();
        for (digest, entry) in &self.entries {
            let signed = signed_parties(state, entry);
            if entry.required.contains(&party) && !signed.contains(&party) {
                awaiting.push((*digest, entry));
            } else if signed.contains(&party) || Party::Member(entry.initiator) == party {
                mine.push((*digest, entry));
            }
        }
        (awaiting, mine)
    }

    /// Is an entry that ALREADY EXISTS waiting on this party? The door's check
    /// for a signer with no account (see `verified_signer`).
    pub fn awaits(&self, digest: &[u8; 32], party: &Party) -> bool {
        self.entries.get(digest).is_some_and(|e| e.required.contains(party))
    }

    pub fn signed_parties_of(&self, state: &State, entry: &PendingEntry) -> Vec<Party> {
        signed_parties(state, entry).into_iter().collect()
    }

    fn prune(&mut self, now_secs: u64) {
        self.entries
            .retain(|_, e| e.created_secs.saturating_add(PENDING_TTL_SECS) > now_secs);
        while self.tombstone_order.len() > MAX_TOMBSTONES {
            if let Some(oldest) = self.tombstone_order.pop_front() {
                self.tombstones.remove(&oldest);
            }
        }
    }

    fn tombstone(&mut self, digest: [u8; 32]) {
        if self.tombstones.insert(digest) {
            self.tombstone_order.push_back(digest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{decline_message, invite_message};
    use crate::block::hex32;

    /// Cross-pin with `ui/src/lib/invite.ts::inviteMessage` and its vitest:
    /// the same fixed inputs must produce these exact bytes on both sides.
    #[test]
    fn invite_message_matches_the_cross_pinned_vector() {
        let msg = invite_message("edet-dev-1", &[0x01; 32], 1_700_000_000, &[0x0a; 16]);
        let hex: String = msg.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "656465742d696e766974652d76310a656465742d6465762d310a303130313031303130313031303130313031303130313031303130313031303130313031303130313031303130313031303130313031303130313031303130310a313730303030303030300a3061306130613061306130613061306130613061306130613061306130613061"
        );
    }

    /// Cross-pin with `ui/src/lib/submit.ts::declineMessage` and its vitest
    /// (`ui/src/lib/__tests__/decline-message.test.ts`): both compute
    /// sha256(digest ++ b"edet-decline"). Pinning the SAME fixed vector
    /// (digest = 0x00..0x1f) here means a change to the marker bytes on either
    /// side breaks a test on that same side, not silently only the TS one.
    #[test]
    fn decline_message_matches_the_cross_pinned_vector() {
        let mut digest = [0u8; 32];
        for (i, b) in digest.iter_mut().enumerate() {
            *b = i as u8;
        }
        assert_eq!(
            hex32(&decline_message(&digest)),
            "ad8631756224977aaa36906d859f311af2caea7635032ce909f8db9bf3438a92"
        );
    }
}
