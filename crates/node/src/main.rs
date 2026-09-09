//! edet-node CLI.
//!
//!   edet-node demo                     three-replica agreement demo (default)
//!   edet-node dev-phrases [--members M]
//!                                      print the founders' recovery phrases
//!   edet-node malachite testnet --home DIR --nodes N
//!                                      write N loopback node homes under DIR/0, DIR/1, ...
//!                                      (needs `malachite`)
//!   edet-node malachite seed-tx --home DIR --nodes N [--debtor D --creditor C --amount A]
//!                                      write a signed demo tx (default debtor 0, creditor 1,
//!                                      amount 10) as every node's seed_tx.json (after `testnet`)
//!                                      — the cluster-agreement test/harness's "submit a
//!                                      transaction" step (needs `malachite`)
//!   edet-node malachite --home DIR [--index I] [--start-height H]
//!              [--client-port P] [--client-peers URL,URL,...] [--cors-port PORT,...]
//!              [--bind-all] [--cluster-token TOKEN] [--allow-unsigned]
//!              [--prune-margin-blocks N]
//!                                      run one such node (DIR/I if --index given, else DIR itself).
//!                                      With --client-port it ALSO serves the browser-client HTTP
//!                                      API (reads/submit/pending/session) against the very state
//!                                      Malachite commits, and gossips submitted transactions to
//!                                      --client-peers (needs `malachite`).
//!                                      A /dns*/ persistent peer in config.toml is resolved ONCE
//!                                      here, by the system resolver, and rewritten to a literal
//!                                      before the transport sees it — so a DNS change needs a
//!                                      restart, and discovery, a /dnsaddr/ peer, a name in the
//!                                      listen address and a name that does not resolve all refuse
//!                                      to boot. Every peer ends in /p2p/<peer id>, and the node
//!                                      dials only the peers whose id is greater than its own: a
//!                                      pair keeps one connection, dialed by the lower id
//!   edet-node malachite export-snapshot --home DIR --out FILE
//!   edet-node malachite import-snapshot --home DIR --from FILE
//!                                      carry a state between nodes when the WAL cannot bridge
//!                                      the gap. A validator down longer than the shortest
//!                                      peer's --prune-margin-blocks is below every peer's
//!                                      history floor and value sync has nothing to serve it;
//!                                      an operator exports from a healthy node and imports
//!                                      here. The import checks the state is a legal state of
//!                                      THIS chain whose commitment a certified block claims;
//!                                      where the file came from is the operator's to vouch
//!                                      for (needs `malachite`)
//!   edet-node malachite keygen --out FILE
//!                                      mint this operator's consensus key at mode 0600 and print
//!                                      its PUBLIC half, which is the CONSENSUSKEYHEX field the
//!                                      genesis author needs, and the peer id peers list you by.
//!                                      Refuses to overwrite. The private half is never printed
//!                                      (needs `malachite`)
//!   edet-node malachite init --home DIR --genesis FILE --key FILE
//!              --listen MULTIADDR [--peers MULTIADDR/p2p/PEERID,...] [--moniker NAME]
//!              [--metrics-port P] [--dev]
//!                                      write this operator's node home from a real genesis, their
//!                                      own key and real addresses — the step between
//!                                      `genesis init` and running a validator. Refuses a key the
//!                                      genesis does not name as a consensus key, a member key on
//!                                      the host, a harness chain id without --dev, a name in the
//!                                      listen address, and a peer without the /p2p/ id keygen
//!                                      printed for it or with one the genesis does not carry;
//!                                      peers may be names (needs `malachite`)
//!   edet-node genesis init --chain-id ID --out FILE [--dev]
//!              --validator ADDR:MEMBERKEYHEX:CONSENSUSKEYHEX:POWER [--validator ... ]
//!                                      author a real genesis.json (see `edet-node genesis init`'s own documentation):
//!                                      addresses must run 0,1,2,... with no gaps, each pubkey a
//!                                      32-byte Ed25519 key in hex, no duplicate keys, at least one
//!                                      validator with power > 0, and chain-id must not start with
//!                                      "edet-dev" — nor may any validator use a published dev key —
//!                                      unless --dev is given (needs `malachite`)

use edet_kernel::constants::EPOCH_SECS;
use edet_node::block::{hex32, Block, SignedTx};
use edet_node::replica::Replica;
use edet_state::types::Party;
use edet_state::{tx::Tx, State};

fn key(n: u8) -> [u8; 32] {
    [n; 32]
}

fn genesis() -> Result<State, Box<dyn std::error::Error>> {
    let mut st = State::default();
    for i in 0..5u8 {
        st.add_underwriter(vec![key(i + 1)], 25_000.0)?;
    }
    Ok(st)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("dev-phrases") => dev_phrases(&args[1..]),
        Some("malachite") => malachite(&args[1..]),
        Some("genesis") => genesis_cmd(&args[1..]),
        _ => demo(),
    }
}

#[cfg(feature = "malachite")]
fn malachite(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use edet_node::block::{dev_seed, sign_tx};
    use edet_node::engine_context::EdetHeight;
    use edet_node::engine_node::{write_seed_tx, write_testnet, ClientApi, EdetApp};
    use edet_state::tx::Tx;
    use malachitebft_app_channel::app::node::Node;

    match args.first().map(String::as_str) {
        Some("testnet") => {
            let rest = &args[1..];
            let home = arg(rest, "--home").ok_or("malachite testnet needs --home DIR")?;
            let nodes: usize = arg(rest, "--nodes").unwrap_or("1").parse()?;
            write_testnet(std::path::Path::new(home), nodes)?;
            println!("wrote {nodes} node home(s) under {home}");
            Ok(())
        }
        Some("seed-tx") => {
            let rest = &args[1..];
            let home = arg(rest, "--home").ok_or("malachite seed-tx needs --home DIR")?;
            let nodes: usize = arg(rest, "--nodes").ok_or("malachite seed-tx needs --nodes N")?.parse()?;
            let debtor: u8 = arg(rest, "--debtor").unwrap_or("0").parse()?;
            let creditor: u8 = arg(rest, "--creditor").unwrap_or("1").parse()?;
            let amount: f64 = arg(rest, "--amount").unwrap_or("10").parse()?;
            let tx = Tx::Accept {
                debtor: Party::Member(debtor as u64),
                creditor: Party::Member(creditor as u64),
                amount,
                maturity_epochs: 30,
                arb: None,
            };
            // The malachite engine runs on real wall-clock time (`EdetApp::
            // start`'s `time_source`), so by the time this seed tx is
            // applied the genesis's epoch 0 has already advanced to
            // `real_unix_secs / EPOCH_SECS` — a small fixed `not_after_epoch`
            // like 30 would already be expired. Anchor the window to NOW,
            // at the widest legal margin (`MAX_TX_LIFETIME_EPOCHS`), which
            // comfortably covers the seconds/minutes between writing this
            // file and the node actually proposing it.
            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let not_after_epoch = now_secs / EPOCH_SECS + edet_kernel::constants::MAX_TX_LIFETIME_EPOCHS;
            let stx = sign_tx(
                edet_node::block::DEV_CHAIN_ID,
                tx,
                edet_node::block::counter_nonce(0),
                not_after_epoch,
                &[dev_seed(debtor), dev_seed(creditor)],
            )
            .map_err(|e| format!("failed to sign the seed tx: {e}"))?;
            write_seed_tx(std::path::Path::new(home), nodes, &stx)?;
            println!("wrote a signed Accept({debtor} -> {creditor}, {amount}) as seed_tx.json for {nodes} node(s) under {home}");
            Ok(())
        }
        // The founding ceremony's node side. Every operator runs `keygen` and
        // sends the hex to the genesis author; the author runs `genesis init`
        // and sends the file back; every operator runs `init`. Nothing is
        // hand-written, which is what the ceremony had before these two: a
        // `config.toml` and a `priv_validator_key.json` per operator, copied
        // from a testnet home that names published dev keys and 127.0.0.1.
        Some("keygen") => {
            let rest = &args[1..];
            let out = arg(rest, "--out").ok_or("malachite keygen needs --out FILE")?;
            let public = edet_node::engine_node::keygen(std::path::Path::new(out))?;
            let peer_id = edet_node::engine_node::peer_id_of_public(&public)?;
            println!("wrote a fresh consensus key to {out} (mode 0600). The private half is not printed.");
            println!("{}", edet_node::block::hex32(&public));
            println!(
                "hand this hex to the genesis author as the CONSENSUSKEYHEX field of your --validator entry. It \
                 is NOT your member key: that one signs Accept, Settle and DeclareSupply and never goes on a \
                 server."
            );
            println!("{peer_id}");
            println!(
                "and this peer id to the operators who will list you: their --peers entry for this validator ends \
                 in /p2p/{peer_id}. A pair of validators keeps one connection, dialed by the lower peer id."
            );
            Ok(())
        }
        Some("init") => {
            let rest = &args[1..];
            let home = arg(rest, "--home").ok_or("malachite init needs --home DIR")?;
            let genesis = arg(rest, "--genesis").ok_or("malachite init needs --genesis FILE")?;
            let key = arg(rest, "--key").ok_or("malachite init needs --key FILE")?;
            let listen = arg(rest, "--listen").ok_or(
                "malachite init needs --listen MULTIADDR, e.g. /ip4/0.0.0.0/tcp/26600 — the interface this \
                 validator binds",
            )?;
            let peers: Vec<String> = arg(rest, "--peers")
                .unwrap_or("")
                .split(',')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect();
            let metrics_port: usize = arg(rest, "--metrics-port").unwrap_or("29500").parse()?;
            edet_node::engine_node::init_home(
                std::path::Path::new(home),
                std::path::Path::new(genesis),
                std::path::Path::new(key),
                listen,
                &peers,
                arg(rest, "--moniker"),
                metrics_port,
                rest.iter().any(|a| a == "--dev"),
            )?;
            println!(
                "wrote {home}/config/{{config.toml,genesis.json,priv_validator_key.json}} — listening on {listen}, \
                 {} peer(s), discovery off",
                peers.len()
            );
            println!("run it with the RELEASE build: `just node-release` then `target/release/edet-node malachite --home {home}`");
            Ok(())
        }
        // The way back for a node below every peer's history floor. Value
        // sync serves what its peers still hold; past their
        // `--prune-margin-blocks` nobody holds it, and an operator carries the
        // state instead.
        Some("export-snapshot") => {
            let rest = &args[1..];
            let home = arg(rest, "--home").ok_or("malachite export-snapshot needs --home DIR")?;
            let out = arg(rest, "--out").ok_or("malachite export-snapshot needs --out FILE")?;
            let height =
                edet_node::engine_malachite::export_snapshot(std::path::Path::new(home), std::path::Path::new(out))?;
            println!("wrote the snapshot at height {height} to {out}, with the block and certificate that certify it");
            Ok(())
        }
        Some("import-snapshot") => {
            let rest = &args[1..];
            let home = arg(rest, "--home").ok_or("malachite import-snapshot needs --home DIR")?;
            let from = arg(rest, "--from").ok_or("malachite import-snapshot needs --from FILE")?;
            let height =
                edet_node::engine_malachite::import_snapshot(std::path::Path::new(home), std::path::Path::new(from))?;
            println!("installed the snapshot at height {height}; this node will start there and sync forward");
            println!(
                "what was checked: the state is a legal state of THIS chain and its commitment is what a certified \
                 block claims. WHERE THE FILE CAME FROM is yours to vouch for."
            );
            Ok(())
        }
        _ => {
            let home = arg(args, "--home")
                .ok_or("malachite needs --home DIR (or `malachite testnet --home DIR --nodes N` first)")?;
            let mut home_dir = std::path::PathBuf::from(home);
            if let Some(idx) = arg(args, "--index") {
                home_dir = home_dir.join(idx);
            }
            let start_height: Option<u64> = arg(args, "--start-height").and_then(|s| s.parse().ok());

            // The browser-client HTTP API, opt-in: without `--client-port` this
            // stays a pure consensus process (what the cluster harness
            // runs), observable only through `status.json`.
            let client = match arg(args, "--client-port") {
                None => None,
                Some(p) => {
                    let bind_all = args.iter().any(|a| a == "--bind-all");
                    // The whole client API goes onto every interface, not
                    // just the gossip routes: `/tx`, `/pending/sign` and the
                    // reads carry no token and never did, and `/p2p/*` calls
                    // the same handlers they do. So the token is a perimeter
                    // on one surface rather than the thing standing between
                    // the internet and the node, and the sentence says so —
                    // an operator told only "set --cluster-token" would read
                    // the rest of the port as guarded.
                    if bind_all {
                        eprintln!(
                            "WARNING: --bind-all puts the whole client API on every interface — plain HTTP, no \
                             transport security of its own. Terminate TLS in front of it and let nothing else \
                             reach the port. --cluster-token additionally pins the tx-gossip /p2p/* surface to \
                             your peers; without it that surface trusts loopback and the configured peer \
                             addresses, which is a thin claim on an open bind."
                        );
                    }
                    // Harness-only, same hatch (and same warning) as
                    // `serve --allow-unsigned`; see `ClientApi`.
                    let allow_unsigned = args.iter().any(|a| a == "--allow-unsigned");
                    if allow_unsigned {
                        eprintln!(
                            "WARNING: --allow-unsigned accepts transactions without signature verification (harness only); peers will vote every block built from them invalid"
                        );
                    }
                    Some(ClientApi {
                        port: p.parse()?,
                        allow_unsigned,
                        // Slot-indexed, this node's own slot included and
                        // ignored — same shape as `serve --peers`.
                        peers: arg(args, "--client-peers")
                            .unwrap_or("")
                            .split(',')
                            .filter(|s| !s.is_empty())
                            .map(String::from)
                            .collect(),
                        cors_ports: arg(args, "--cors-port")
                            .unwrap_or("")
                            .split(',')
                            .filter_map(|s| s.parse().ok())
                            .collect(),
                        bind_all,
                        cluster_token: arg(args, "--cluster-token").map(String::from),
                        // An assertion about the deployment, not a guess: only
                        // sound when a reverse proxy OVERWRITES the header and
                        // nothing else can reach this port. Off by default,
                        // because a trusted header any client can set is a
                        // fresh rate-limit bucket per request.
                        trust_forwarded_for: args.iter().any(|a| a == "--trust-forwarded-for"),
                    })
                }
            };

            let mut app = EdetApp::at(home_dir, start_height.map(EdetHeight::new), client);
            // **How long this node may be down and still rejoin from a peer.**
            // Below `2 * snapshot_interval` the WAL could be cut inside the
            // last snapshot's own interval, leaving a hole no peer can serve,
            // so the replica floors it there whatever this says — refused here
            // instead, where the message can explain the number.
            if let Some(margin) = arg(args, "--prune-margin-blocks") {
                let margin: u64 = margin.parse()?;
                if margin < 256 {
                    return Err(format!(
                        "--prune-margin-blocks {margin} is below two snapshot intervals (256 blocks), which is the \
                         least a syncing peer can use: it needs a snapshot AND every block above it"
                    )
                    .into());
                }
                app.prune_margin_blocks = margin;
            }

            // The engine logs through `tracing`; without a subscriber
            // installed those records go nowhere, which on a validator means
            // no operational visibility at all. Installed here rather than
            // via the vendored test CLI's `logging::init` — see
            // `engine_node::write_node_home` for why that crate is gone.
            // `RUST_LOG` overrides the default when an operator needs more.
            //
            // `with_ansi(false)` is not cosmetic: a validator's output goes to
            // a file or journald, not a terminal, and colour codes there are
            // corruption — they land *inside* field names, so `height=1` is
            // written as `height<esc>=<esc>1` and anything reading the log
            // (an operator's grep, an alert rule, `tests/malachite_byzantine.rs`'s
            // conflicting-certificate parser) silently matches nothing. That
            // test caught exactly this when the vendored `LogFormat::Plaintext`
            // was replaced; it is the format contract, so keep it plain.
            tracing_subscriber::fmt()
                .with_ansi(false)
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
                )
                .init();

            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
            rt.block_on(app.run()).map_err(|e| format!("malachite node error: {e:#}"))?;
            Ok(())
        }
    }
}

#[cfg(not(feature = "malachite"))]
fn malachite(_args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("`malachite` needs the engine feature: cargo run -p edet-node --features malachite -- malachite ...");
    std::process::exit(2);
}

/// Author a real `genesis.json` — the one place a chain-id/validator-set
/// is supposed to come from OUTSIDE this crate's dev harnesses. Everything
/// this command validates (address order, key shape, dedup, dev-key
/// tripwire) `EdetApp::start`/`genesis_state` will ALSO check at boot — this
/// command exists so the ceremony catches a mistake immediately, at
/// authoring time, with a file open and an operator still at the keyboard,
/// rather than as a cryptic refusal weeks later on a validator no one
/// remembers configuring.
///
/// # The founding ceremony
///
/// Two independent things are agreed at a founding, and they are not the same
/// set and need not overlap: **who orders transactions** (the validators) and
/// **where credit comes from** (the founding underwriters). Underwriting is an
/// economic position rather than an operational one — an underwriter need not
/// run a validator, and a validator need not underwrite.
///
/// **Validator keys.** Each operator generates their own Ed25519 keypair
/// OFFLINE; whoever controls that seed controls that validator's vote.
/// Operators record their intended address and public key and nothing else —
/// never the seed, never `priv_validator_key.json`. Public keys are collected
/// and cross-checked out of band, because a misassigned index misassigns every
/// later validator's identity. That is what the ordering, duplicate, hex,
/// zero-power and dev-key checks below are for.
///
/// **The founding underwriters** are the more consequential half, because that
/// is where the community's credit comes from and where nearly all of it will
/// still come from later. Each declared supply is an accepted liability: if the
/// members it backs fail, that much of the loss is the underwriter's, inherited
/// as debtor. It should name nobody it does not honestly describe and no amount
/// anybody cannot bear. A community may choose to underwrite NOTHING and it
/// will work — every obligation is uninsured and borne bilaterally, an ordinary
/// mutual-credit network — but it can never leave that state: with no
/// underwriter, settled trade writes no stake, every capacity stays at zero,
/// and the largest declarable supply stays at zero, because a declaration is
/// capped by a capacity that is capped by there being no underwriter. Genesis
/// underwriting is not a default; it is the only seed. How to size it is in the
/// paper ("Sizing the founding seed"): peak simultaneous insured credit,
/// never annual volume.
///
/// # Assembly and first boot
///
/// ```text
/// edet-node genesis init \
///   --chain-id <real-chain-id, NOT starting with "edet-dev"> \
///   --out genesis.json \
///   --validator 0:<pubkey_hex>:<power> \
///   --validator 1:<pubkey_hex>:<power> \
///   ...
/// ```
///
/// The coordinator role is disposable. Publish `genesis.json` AND its
/// `sha256sum` over the same out-of-band channel; every operator independently
/// recomputes the hash and confirms it byte for byte before installing the
/// file. A genesis file differing by one byte means those nodes are on
/// different chains and will never agree on anything.
///
/// Before first boot, confirm each data directory is empty and that your own
/// `priv_validator_key.json` public key matches your assigned slot. After first
/// boot, **do not transact until the epoch has caught up to wall-clock** — a
/// fresh chain expires transactions until it does.
#[cfg(feature = "malachite")]
fn genesis_cmd(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use edet_node::engine_node::{is_dev_key, EdetGenesis, EdetGenesisEntry, EdetGenesisUnderwriter};

    match args.first().map(String::as_str) {
        Some("init") => {
            let rest = &args[1..];
            let chain_id = arg(rest, "--chain-id").ok_or("genesis init needs --chain-id ID")?.to_string();
            let out = arg(rest, "--out").ok_or("genesis init needs --out FILE")?;
            let dev = rest.iter().any(|a| a == "--dev");

            if chain_id.is_empty() {
                return Err("--chain-id must not be empty".into());
            }
            if chain_id.starts_with("edet-dev") && !dev {
                return Err(format!(
                    "chain-id \"{chain_id}\" starts with the reserved harness prefix \"edet-dev\" — pass --dev if this really is a throwaway dev/test chain, or pick a real chain id"
                )
                .into());
            }
            // `--dev` is the harness chain, exactly, and nothing else. The
            // boot gate that grants dev concessions — the published salt
            // fallback and the dev-key tripwire — tests `chain_id ==
            // DEV_CHAIN_ID`, not a prefix, because a prefix let plausible real
            // names like `edet-development-fund` silently inherit both. So a
            // `--dev` genesis on any other id writes a file that refuses to
            // boot: dev keys on a chain the gate calls real. Refused here,
            // where the message can say why, rather than at first start.
            if dev && chain_id != edet_node::block::DEV_CHAIN_ID {
                return Err(format!(
                    "--dev writes a harness genesis, which only chain-id \"{}\" may boot (the gate is exact, not a prefix) — drop --dev for a real chain id, or use \"{}\"",
                    edet_node::block::DEV_CHAIN_ID,
                    edet_node::block::DEV_CHAIN_ID
                )
                .into());
            }

            // Collect every `--validator ADDR:MEMBERKEYHEX:CONSENSUSKEYHEX:POWER`
            // in the order given on the command line (not sorted — a caller who
            // lists them out of order gets told exactly that, the same mistake
            // `genesis_state` itself would otherwise only discover at load
            // time).
            //
            // **The consensus key is its own field, and an empty one is how a
            // founder says they run no validator.** It was the member key until
            // one key seated as both makes a validator
            // host compromise was an economic identity compromise. Naming both
            // here is what puts the decision in the ceremony, which is the only
            // place that knows which humans hold which box.
            let mut validators: Vec<EdetGenesisEntry> = Vec::new();
            let mut seen_keys: Vec<[u8; 32]> = Vec::new();
            let mut i = 0;
            while i < rest.len() {
                if rest[i] == "--validator" {
                    let spec = rest
                        .get(i + 1)
                        .ok_or("--validator needs an ADDR:MEMBERKEYHEX:CONSENSUSKEYHEX:POWER argument")?;
                    let parts: Vec<&str> = spec.split(':').collect();
                    if parts.len() != 4 {
                        return Err(format!(
                            "--validator {spec}: expected ADDR:MEMBERKEYHEX:CONSENSUSKEYHEX:POWER (four \
                             ':'-separated fields; leave CONSENSUSKEYHEX empty for a founder who runs no validator)"
                        )
                        .into());
                    }
                    let address: u64 = parts[0]
                        .parse()
                        .map_err(|_| format!("--validator {spec}: {} is not a valid address", parts[0]))?;
                    let public_key = parse_pubkey_hex(parts[1]).map_err(|e| format!("--validator {spec}: {e}"))?;
                    let consensus_key = match parts[2] {
                        "" => None,
                        hex => {
                            Some(parse_pubkey_hex(hex).map_err(|e| format!("--validator {spec}: consensus key: {e}"))?)
                        }
                    };
                    let voting_power: u64 = parts[3]
                        .parse()
                        .map_err(|_| format!("--validator {spec}: {} is not a valid power", parts[3]))?;

                    if consensus_key == Some(public_key) {
                        return Err(format!(
                            "--validator {spec}: the member key and the consensus key are the same key. The \
                             consensus key lives unencrypted on a host that answers the internet and signs a vote a \
                             second; the member key signs Accept, Settle and DeclareSupply. Generate two."
                        )
                        .into());
                    }
                    if voting_power > 0 && consensus_key.is_none() {
                        return Err(format!(
                            "--validator {spec}: voting power with no consensus key. A validator whose signing key \
                             the ledger cannot name is one no certificate can be verified against — give it a \
                             consensus key, or set POWER to 0 to seat a founder who runs no validator"
                        )
                        .into());
                    }

                    let expected = validators.len() as u64;
                    if address != expected {
                        return Err(format!(
                            "--validator {spec}: genesis addresses must run 0,1,2,... in ascending order with no gaps (expected address {expected}, got {address}) — add_genesis_member assigns member ids sequentially in this order, so a gap or reorder here would silently misassign every later validator's identity"
                        )
                        .into());
                    }
                    if seen_keys.contains(&public_key) {
                        return Err(format!(
                            "--validator {spec}: duplicate public key (already used by an earlier --validator)"
                        )
                        .into());
                    }
                    if let Some(ck) = consensus_key {
                        if seen_keys.contains(&ck) {
                            return Err(format!(
                                "--validator {spec}: this consensus key is already in use by an earlier --validator \
                                 (as a member key or a consensus key). One key is one identity."
                            )
                            .into());
                        }
                    }
                    // Both keys, because a real network's security rests on
                    // both and only one of them was checked while they were
                    // the same key.
                    for (what, key) in [("public key", Some(public_key)), ("consensus key", consensus_key)] {
                        if key.is_some_and(|k| is_dev_key(&k)) && !dev {
                            return Err(format!(
                                "--validator {spec}: this {what} is one of edet's own published dev keys (block::dev_seed / block::dev_consensus_seed) — it secures nothing on a real network; pass --dev only if this really is a throwaway dev/test genesis"
                            )
                            .into());
                        }
                    }

                    seen_keys.push(public_key);
                    seen_keys.extend(consensus_key);
                    validators.push(EdetGenesisEntry {
                        address: edet_node::engine_context::EdetAddress(address),
                        public_key,
                        consensus_key,
                        voting_power,
                    });
                    i += 2;
                } else {
                    i += 1;
                }
            }

            if validators.is_empty() {
                return Err("genesis init needs at least one --validator ADDR:PUBKEYHEX:POWER".into());
            }

            // The founding underwriters (§Adoption), and this is the more
            // consequential of the two lists. The validator set decides who
            // orders transactions; this one decides where the community's
            // credit comes from — and by §Standing, where nearly all of it will still
            // come from later, since admitting underwriters from within
            // reallocates credit without creating any.
            let mut underwriters: Vec<EdetGenesisUnderwriter> = Vec::new();
            let mut i = 0;
            while i < rest.len() {
                if rest[i] == "--underwriter" && i + 1 < rest.len() {
                    let spec = &rest[i + 1];
                    let (addr_s, supply_s) = spec
                        .split_once(':')
                        .ok_or_else(|| format!("--underwriter {spec}: expected ADDR:SUPPLY"))?;
                    let address: u64 = addr_s
                        .parse()
                        .map_err(|_| format!("--underwriter {spec}: ADDR must be a whole number"))?;
                    let supply: f64 = supply_s
                        .parse()
                        .map_err(|_| format!("--underwriter {spec}: SUPPLY must be a number"))?;
                    if supply <= 0.0 || !State::amount_representable(supply) {
                        return Err(format!(
                            "--underwriter {spec}: a declared supply is an accepted liability and must be positive \
                             and nameable by the ledger (at most {}); omit the flag entirely to seat a member who \
                             underwrites nothing",
                            State::from_minor(edet_kernel::constants::MAX_AMOUNT_MINOR)
                        )
                        .into());
                    }
                    if !validators.iter().any(|v| v.address.0 == address) {
                        return Err(format!(
                            "--underwriter {spec}: address {address} is not one of the --validator entries. An \
                             underwriter must be a member of the community it stands behind — seat them with \
                             --validator {address}:PUBKEYHEX:0 if they run no validator, since underwriting is an \
                             economic position and running a validator is an operational one"
                        )
                        .into());
                    }
                    if underwriters.iter().any(|u| u.address.0 == address) {
                        return Err(format!("--underwriter {spec}: address {address} named twice").into());
                    }
                    underwriters.push(EdetGenesisUnderwriter {
                        address: edet_node::engine_context::EdetAddress(address),
                        supply,
                    });
                    i += 2;
                } else {
                    i += 1;
                }
            }
            // The roll as a SUM, asked here as well as at boot: a ceremony
            // seats every founder at once, and each one being inside the
            // ceiling says nothing about what they declare between them.
            edet_node::engine_node::check_seed_total(underwriters.iter().map(|u| u.supply))
                .map_err(|e| format!("genesis init: {e}"))?;
            if underwriters.is_empty() {
                // Legal, and it works — every obligation uninsured, borne
                // bilaterally, an ordinary mutual-credit network. But it is a
                // state the community can never LEAVE: with no supply there is
                // no source arc, so no capacity, so nothing conferrable, so no
                // settlement stakes anything, so nobody can ever declare a
                // supply. Warned here rather than refused, because it is a
                // choice a community is entitled to make and must make knowing
                // that no amount of subsequent trading substitutes for a seed.
                eprintln!(
                    "warning: no --underwriter named. This community will underwrite NOTHING: every obligation \
                     uninsured, every capacity permanently zero, and no member will ever be able to declare a \
                     supply. Zero is absorbing — see the paper's §Adoption."
                );
            }
            if !validators.iter().any(|v| v.voting_power > 0) {
                return Err("genesis init needs at least one validator with power > 0 — an all-zero-power validator set can never reach quorum".into());
            }
            // The BFT floor, at the one moment the chain id is in hand.
            //
            // The state machine's own `MIN_VALIDATORS` is 1, because it cannot
            // tell a throwaway chain from a real one, so a real chain could be
            // FOUNDED with a single validator and the safety argument every
            // certificate rests on would be vacuous from block 1. `3f + 1`
            // tolerates `f` faults, so 4 is the smallest set that tolerates
            // one — and a set that tolerates none is a single point of failure
            // wearing a quorum.
            let powered = validators.iter().filter(|v| v.voting_power > 0).count();
            if !dev && powered < edet_kernel::constants::MIN_VALIDATORS_REAL_CHAIN {
                return Err(format!(
                    "genesis init: chain \"{chain_id}\" names {powered} validator(s) with voting power, and a real \
                     chain needs at least {} (BFT tolerates f faults out of 3f+1, so 4 is the smallest set that \
                     tolerates one). Pass --dev for a throwaway chain, or seat more validators.",
                    edet_kernel::constants::MIN_VALIDATORS_REAL_CHAIN
                )
                .into());
            }

            // A fresh 32-byte state-root salt, from the OS CSPRNG
            // (the paper's §Implementation). Drawn here rather
            // than defaulted at load time so it is written into the genesis
            // file every validator receives — they must all hold the SAME
            // value or they compute different roots and the chain cannot
            // agree. A real chain must never inherit the published dev salt:
            // that would let anyone holding one inclusion proof brute-force
            // the sibling hashes travelling with it and read the records
            // next to the one being proved.
            let mut root_salt = [0u8; 32];
            getrandom::getrandom(&mut root_salt)
                .map_err(|e| format!("could not draw a state-root salt from the OS RNG: {e}"))?;

            // Two more things the ceremony decides and the state machine
            // cannot: the validator floor the ledger holds every removal path
            // to (`Params::min_validators`), and whether amounts are sealed
            // to the parties. Both written into the file, because a genesis
            // file is the ceremony's record.
            let open_amounts = rest.iter().any(|a| a == "--open-amounts");
            let genesis = EdetGenesis {
                validators,
                underwriters,
                chain_id: chain_id.clone(),
                root_salt: Some(root_salt),
                min_validators: Some(if dev {
                    edet_kernel::constants::MIN_VALIDATORS as u64
                } else {
                    edet_kernel::constants::MIN_VALIDATORS_REAL_CHAIN as u64
                }),
                seal_amounts: Some(if dev || open_amounts { 0.0 } else { 1.0 }),
            };
            let json = serde_json::to_string_pretty(&genesis)?;
            std::fs::write(out, json)?;
            let seed: f64 = genesis.underwriters.iter().map(|u| u.supply).sum();
            println!(
                "wrote genesis for chain \"{chain_id}\" to {out}: {} validator(s), {} underwriter(s), seed {seed}",
                genesis.validators.len(),
                genesis.underwriters.len()
            );
            println!(
                "size the seed to PEAK SIMULTANEOUS insured credit, never annual volume — it is reserved, \
                 released and reserved again, turning over once per settlement term: 12x a year at the \
                 30-epoch maturity floor, 4x on 90-day terms. Err low."
            );
            Ok(())
        }
        _ => Err("usage: edet-node genesis init --chain-id ID --out FILE [--dev] \
                  --validator ADDR:MEMBERKEYHEX:CONSENSUSKEYHEX:POWER ... [--underwriter ADDR:SUPPLY ...]"
            .into()),
    }
}

#[cfg(not(feature = "malachite"))]
fn genesis_cmd(_args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("`genesis` needs the engine feature: cargo run -p edet-node --features malachite -- genesis ...");
    std::process::exit(2);
}

/// Decode exactly 32 bytes of hex into an Ed25519 public key, rejecting
/// anything that is not valid hex, not 32 bytes, or not a point Ed25519
/// actually accepts (`VerifyingKey::from_bytes` rejects a torsion/garbage
/// encoding the same way signature verification later would) — a bad key
/// pasted into a genesis file must fail HERE, not at first boot.
#[cfg(feature = "malachite")]
fn parse_pubkey_hex(s: &str) -> Result<[u8; 32], String> {
    if s.len() != 64 {
        return Err(format!("expected a 64-character hex-encoded 32-byte public key, got {} characters", s.len()));
    }
    let mut out = [0u8; 32];
    for (i, chunk) in out.iter_mut().enumerate() {
        let byte_str = &s[i * 2..i * 2 + 2];
        *chunk = u8::from_str_radix(byte_str, 16).map_err(|_| format!("{s} is not valid hex"))?;
    }
    ed25519_dalek::VerifyingKey::from_bytes(&out).map_err(|e| format!("{s} is not a valid Ed25519 public key: {e}"))?;
    Ok(out)
}

/// Print the dev founders' recovery phrases — what the launchers show so a
/// first-run user restores a founder through the ordinary onboarding flow.
fn dev_phrases(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let members: u8 = arg(args, "--members").unwrap_or("5").parse()?;
    for i in 0..members {
        println!("founder {i}: {}", edet_node::block::dev_phrase(i));
    }
    Ok(())
}

#[cfg_attr(not(feature = "malachite"), allow(dead_code))]
fn arg<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

/// Three replicas over an identical block stream land on one state commitment.
fn demo() -> Result<(), Box<dyn std::error::Error>> {
    let names = ["ada", "bo", "cato", "dee", "eze"];
    let mut replicas = vec![Replica::new(genesis()?), Replica::new(genesis()?), Replica::new(genesis()?)];

    for round in 0..6u64 {
        let mut txs = Vec::new();
        // The block this round lands in closes epoch `round + 1`; a window
        // of +30 keeps every tx comfortably valid without approaching
        // MAX_TX_LIFETIME_EPOCHS.
        let not_after_epoch = round + 1 + edet_kernel::constants::MAX_TX_LIFETIME_EPOCHS;
        for w in 0..5u64 {
            let (buyer, seller) = (w, (w + 1) % 5);
            let signers = vec![key(buyer as u8 + 1), key(seller as u8 + 1)];
            let tx_seq = round * 10 + w * 2;
            txs.push(SignedTx {
                tx: Tx::Accept {
                    debtor: Party::Member(buyer),
                    creditor: Party::Member(seller),
                    amount: 40.0,
                    maturity_epochs: 30,
                    arb: None,
                },
                nonce: edet_node::block::counter_nonce(tx_seq),
                not_after_epoch,
                signers: signers.clone(),
                signatures: vec![],
            });
            let cid = round * 5 + w;
            txs.push(SignedTx {
                tx: Tx::Settle { contract: cid, amount: 40.0 },
                nonce: edet_node::block::counter_nonce(tx_seq + 1),
                not_after_epoch,
                signers,
                signatures: vec![],
            });
        }
        // Every replica in this demo starts from the identical genesis
        // and has applied the identical block stream so far, so any one of
        // them reports the same app_hash the others expect next.
        let app_hash = replicas[0].app_hash();
        let block = Block { height: round + 1, time_secs: (round + 1) * EPOCH_SECS, app_hash, txs };
        for r in &mut replicas {
            // Demo blocks are locally built and unsigned (a state-machine
            // showcase, below the signature layer) — the trusted hatch.
            r.commit_block_unchecked(&block)?;
        }
    }

    let hashes: Vec<String> = replicas.iter().map(|r| hex32(&r.app_hash())).collect();
    let agreed = hashes.windows(2).all(|w| w[0] == w[1]);
    let st = &replicas[0].state;
    println!(
        "height {}  epoch {}  insured {:.2}  utilisation {:.1}%  replicas-agree {}",
        replicas[0].height,
        st.epoch,
        st.committed_total(),
        100.0 * st.utilisation(),
        agreed
    );
    println!("state commitment: {}…", &hashes[0][..16]);
    for (i, id) in (0..5u64).enumerate() {
        if let Some(m) = st.members.get(&id) {
            println!(
                "{:5}  cap {:8.2}  confer {:8.2}  debt {:8.2}",
                names[i],
                st.capacity_of(id),
                st.conferrable(id),
                m.debt_out
            );
        }
    }
    Ok(())
}
