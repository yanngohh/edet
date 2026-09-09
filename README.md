# edet v0.7.0

Mutual credit for communities of mutual producers. Debt originates peer-to-peer
at the point of sale, discharges by circulating onward as its bearer delivers
value, and cannot be held: no token, no gas, no hoardable store of value. What a
member accumulates is *capacity*, a bounded, non-transferable ceiling on how
much the community will let them owe.

One equation decides admission, reputation, credit limits and Sybil resistance:

> **Capacity is the maximum flow into an account from the community's
> underwriters, across directed stakes of what creditors have placed. Nobody can
> stake beyond what they may confer. Outstanding credit reserves its flow.**

For a member: **you may owe the community as much as it has put behind you.**

A ledger is one total order over transitions, one state and one validator set,
replicated under BFT consensus by validators the community operates, in the
same binary its members run: validator, full and light are configurations, not
products. One ledger carries as many communities as it likes, because the
boundary between two is the absence of a stake path rather than a chain. It is
designed and measured for communities up to the order of a hundred thousand
accounts on one box; a larger ledger is an open problem the paper states, and
no figure here comes from validators more than a loopback apart.

## The model

**There is no membership.** No status, attestation, probation, sponsorship,
trial tier, admission or creation transition. A row is seated by the first
bonded trade that names its key, which prices it like every other row and leaves
nobody to approve it; a key that has done nothing is worth zero.

**An underwriter is a member who has declared a supply**: the credit they stand
behind, an accepted liability rather than a rank. A supply bounds the aggregate,
so backing twenty accounts risks exactly what was declared, and capacity draws
supply only from underwriters outside the set being measured, so a coalition
cannot underwrite itself. Every supply arc is seated by a ceremony, genesis or
an endorsed amendment, so the underwriter roll is the external seed; a
declaration can only ever be lowered. Governance reads the same figure: assent
weight is a share of the external seed for every proposal kind. Members propose,
the ceremony enacts.

**Standing is evidence of having owed and paid.** `stake(c, d)` runs from
creditor `c` to debtor `d`, is written only at settlement, is capped by what the
creditor may confer, and is a peak rather than a sum, so a wash loop cannot
accumulate. Selling earns none of it: 2000 settled sales leave the seller at
zero, and one honoured purchase confers standing at once. Capacity grows insured
across creditors and uninsured within one, because the cut sums every honoured
relationship while a single stake is a peak.

**Within capacity an obligation is insured**: it reserves flow along the path it
used, and on default the underwriters whose supply carried it become the
creditor's debtors while the defaulter's debt moves to them. Nobody is released
and nothing is created. Beyond capacity a creditor may still lend, uninsured: it
reserves nothing, triggers no substitution, and the creditor bears it alone.
That is what dissolves the bootstrap: first trades are uninsured, they settle,
they write stakes, and capacity is their residue.

**Insurance has a term.** A claim is insured only within the insured horizon of
its acceptance, a year at genesis and governed between the maturity floor and
the maximum horizon. A longer maturity books uninsured however much capacity
carries it, and an extension past the horizon drops the insurance, both on the
creditor's own signature. The base is the acceptance, which every debtor swap
inherits, so no chain of extensions rolls it; a claim kept insured longer is
settled and re-accepted against the current cut. An underwriter's exposure is
bounded in amount by its supply and in time by a term the underwriters, who are
the electorate, set in advance.

**Rings of defaults net themselves.** A sale nets bilaterally before it creates;
the epoch sweep nets longer rings by their minimum with no signature, because
every hop is in default, so nobody is paid early. Each hop is an ordinary
discharge and earns standing.

**A community is a region of the stake graph.** Nothing in the state names one.
Two co-ops on one chain stay separate while nobody trades across and join the
moment somebody does, with no transition, bridge or ceremony.

`paper/edet.pdf` is the specification. `CLAUDE.md` carries what neither the
paper nor the code states: the settled decisions, the standing cautions, and
the rules this code has already paid for.

## Why it is Sybil-proof

> **The free-signature bound.** Every ledger event is a signature, and
> signatures are free. No quantity computed from ledger events alone can seed
> insured credit: the seed must be a commitment by somebody with something to
> lose outside the ledger.

Capacity is a cut, so the bound applies to any coalition as a whole: splitting
across identities gains nothing, since stakes internal to a set never cross its
boundary. Nothing is detected and no heuristic is tuned. Every row is a probe
against an independent max-flow oracle, gated (the paper's §Security;
`sim/suites/security.py`):

| claim | measured |
|---|---|
| minting identities confers nothing | a fresh key has no incident stakes, and a supply is seated only by a ceremony |
| wash trading confers nothing | 2000 cycles between accounts nobody backs → capacity **0** |
| one supply is lent once | one underwriter of 2500 backing k = 2, 5, 20, 100 debtors → **2500** in total |
| a malicious underwriter is bounded | 20 sybils, 50 rounds, with back-staking → **2500**, the declared supply |
| a coalition cannot underwrite itself | 8 rounds of self-appointment → pinned at external backing |
| nor insure the accounts it backs | 12 joiners, 12 sybils, a seed of 100 → **100** insurable, against 2048× under a rule that lets a member declare against conferred capacity |
| no double-draw across creditors | 50 creditors lending to one debtor → the total equals the cut |
| usage cannot seed insured credit | 1000 settled trades, no underwriters → no stake written, cut **0** |
| attacking costs what participating costs | capacity extracted is linear in real backing, at **1.00×**; the same ratio for the WRITE surface, at k = 3, 7, 12 |
| a row is a stock priced in the cut | a seat holds one bond unit of the sponsor's reach and nothing returns it: **25** rows behind one edge of 500.00, **250** behind 5,000.00, unchanged over 400 epochs of daily renewal, +25 for a second backer of 500.00. A community seats its seed over the unit and then nobody — 375 for a seed of 7,500.00 |

Nothing is assumed about membrane quality, registry integrity or the honesty of
any account. What is assumed is that underwriters can bear what they declared:
the protocol enforces the consequences and cannot verify the promise. Every
statement is scoped to one ledger, which is why communities that expect to
trade share an order rather than bridge, and why no proof or foreign state root
may authorise a transition here.

Capacity is consensus-critical: integer Dinic over minor units, ordered
iteration, no floating point in the path, bit-identical on every replica and
invariant under input permutation. One acceptance costs 0.4 ms at 1,000
accounts and 9.7 ms at 20,000, the write gate's cut being memoised. A committed
block costs the invariant re-audit over the whole set family, and a validator
computes almost none of it: a verified cut is a lower bound until an edge, a
supply or the id space falls, and an insured obligation stores the flow it drew,
which the audit verifies as a witness. At 20,000 accounts with 200 underwriters
the definition computes **201** sets in 2.13 s where the commit path computes
none and pays **28.4 ms**. The definition is linear in the underwriter count;
the witness removes that term, so a validator's audit scales with the graph
(the paper's §Implementation, `just cost`).

## Layout

| Path | Contents |
|---|---|
| `paper/` | **The specification**: model, security argument, genesis and seed-sizing guidance, implementation, verification. LaTeX sources and the built PDF; constants are generated from the kernel |
| `crates/kernel` | The deterministic economic kernel: integer max-flow capacity, exact reservation and release, decay, the cascade's waterfilling, the advisory risk score, the genesis constants. Pure functions, no I/O, no clock |
| `crates/state` | The state machine: twenty transitions, seven invariants re-checked over sets on the commit path, named error codes, the Merkle state root with inclusion proofs. Engine-agnostic: `apply(state, tx, signers, time)` |
| `crates/node` | Blocks and their codec, the durable store (WAL + snapshots, crash recovery), the mempool, deterministic replicas; the client HTTP API and per-viewer authenticated reads behind `--features serve`; the embedded Malachite engine behind `--features malachite` |
| `ui/` | Svelte + SMUI wallet (contracts, community, governance, support, requests, onboarding; i18n ×6; BIP39 recovery phrase and encrypted identity vault), speaking HTTP to whichever node the member selects. It computes the digest it signs (`lib/txdigest.ts`, cross-pinned by `just tx-digest-check`), so a node can withhold and omit but never forge or choose what a member signs; the chain id a signature binds to is the network's declaration, typed beside the URL for `custom`, and without one the wallet refuses to sign. `/tx` returns the outcome hash and the wallet polls it, so a refusal at commit reaches the member. The acceptance rule is device-only policy, and a member's own price for a counterparty (`lib/pricing.ts`) is shown beside the ledger's score, never in its place. It decides for as long as the app is RUNNING — in front of the member or behind, where background mode holds the page up (`lib/background.ts`) — and for no longer: the seed is in the page, so a closed app signs nothing. Buying is the one trade a member records; the seller signs it under Requests. A key with no account yet cannot be charged for a pending-pool entry, so its first purchase reaches the pool only on the seller's signed invitation, which the seller's "pay me" QR carries (`lib/invite.ts`, charged to the inviter and bounded per inviter); without one it is handed to the seller as a code (`lib/offer.ts`), whose wallet recomputes the digest, checks the signature, co-signs and submits both |
| `src-tauri/` | Tauri v2 desktop and mobile client: a signing wallet, not a validator. No consensus, no ledger; the Rust half holds the identity-vault device key in the OS keychain (Android Keystore on mobile), and keeps the WebView alive when the member looks elsewhere — a tray the window hides into on the desktop, a foreground service and a re-resumed WebView on Android (`edet-background`) |
| `crates/swarm` | **The strategy swarm**: ten archetypes (honest, deadbeat, wash ring, sybil farm, griefer, governance coalition, panic exiter, honest-then-evil sleeper, late defaulter, hoarder) over the real transition function, judged by the real audit after every transition. A strategy emits intents and one adapter builds every envelope; a decline is the pending-signature pool, not a refusal. Every run is one seed and replays bit-identically; `corpus.json` pins twenty-four scenes. It is the tree's one driver: the state suite's `Chain` is its fixtures, and `crates/node` replays a real node's committed blocks through it |
| `sim/` | An independent max-flow oracle in numpy/scipy, the security probes, and the kernel cross-pin fixtures |
| `CLAUDE.md` | The settled decisions, the standing cautions and the rules this code has already paid for |

## Build

```sh
just ci                         # every gate, in order — this is the authoritative list
cargo nextest run --workspace   # kernel + state machine + replication + node
cargo run -p edet-node          # three-replica agreement demo
python3 sim/run.py --fixtures   # the security probes + kernel cross-pin freshness
just swarm                      # the pinned agent-population corpus (in `ci`)
just swarm-search N=256         # fresh seeds, out of band, one seed per thread
just swarm-q2 N=50              # the distributional reading, every figure a ratio to a control
python3 scripts/persona.py attack --families anthropic,gemini  # by hand, never in a gate
just cost                       # the cost tables, per acceptance and per block (not a gate: wall-clock)
just node-release               # the binary a validator runs (release, overflow checks on)
cd paper && pdflatex edet.tex   # the paper (twice for cross-references)

# Desktop client (starts a node beside it; needs the tauri toolchain via nix):
just tauri                      # run the desktop app
just tauri-build                # build the bundle

# Mobile client on a connected device (needs the android SDK/NDK via nix):
just android-apk                # the debug APK, one ABI (default aarch64)
just android-run                # build, install, and follow the device log
# The SIGNED release APK and AAB are CI's, not a recipe's: pushing a `v*.*.*`
# tag runs `.github/workflows/android-release.yaml`, which verifies the
# signature and that R8 kept both plugin classes, then attaches
# `edet-<version>-release.{apk,aab}` to the GitHub Release.

# Local cluster + browser UI (needs `just`, node, npm) — all on the real engine:
just dev                        # N devices: node i + UI i paired on their own ports (default 2)
just malachite-cluster          # N headless validators, no UI
just engine-test                # the engine's integration tests (real processes, real gossip)
just e2e                        # the first-contact journey against a real node

# Reproducible toolchain (Rust 1.90 + python/numpy/scipy) via nix:
nix develop                     # rust build/test shell
nix develop .#sim               # python-only sim shell
nix develop .#tauri             # desktop client toolchain
nix develop .#android           # Android SDK/NDK for the mobile client
nix flake check                 # workspace tests + sim suites
```

`just ci` runs `fmt-check · paper-constants-check · seed-table-check ·
proof-fixture-check · tx-digest-check · view-shape-check · clippy · test ·
engine-check · engine-test · sim · swarm · ui-check · ui-test ·
e2e · tauri-deps · tauri-check · audit`, and GitHub Actions runs `just ci` and
nothing else. **Read its exit code**: `set -o pipefail`, because `just ci | tail`
reports `tail`'s status. `audit` queries a live advisory registry, so it can go
red with no commit behind it; it reads the dependency tree, so a library that
vendors its dependencies carries their code past it; and it exits **2** when a
checker could not run, because green has to mean checked. Both halves need a
tool this repo does not vendor: `nix develop` ships them, otherwise `cargo
install cargo-audit --locked`. Two Rust advisories are carried rather than
fixed, both in the DNS resolver `libp2p` brings in under the pinned Malachite
commit; the recipe prints them with their reason on every run, and the
reachability argument under the more serious one, that no build here enables a
dnssec feature, is checked by the gate.

## Running a node that is reachable

**A validator runs a release build**: `just node-release`, then
`target/release/edet-node malachite --home DIR ...`. Every other launcher in the
justfile is a debug harness on purpose, and the workspace keeps
`overflow-checks` on in release, so an arithmetic overflow is a deterministic
fail-stop every node takes at the same height, never a silent wrap hashed into
the state. The cost figures here and in the paper are release figures.

**A validator's clock is a liveness input.** A proposer's block time may lead a
validator's clock by ten minutes (`block::MAX_FUTURE_SKEW_SECS`) before that
validator refuses to vote, and a refusal a quorum overrides is a fail-stop for
the refuser: it loses its consensus actor. Keep validators on NTP. One ten
minutes slow stops itself; one ten minutes fast is refused by everybody else.

**The client API is plain HTTP with no transport security of its own.** Bearer
session tokens travel in clear and are replayable for their whole TTL; a viewer
signature is admitted once inside its 60-second window, and a second
presentation answers `401 replayed viewer signature`. The default bind is
loopback, tolerable for a node beside its member's own wallet; anything
reachable from elsewhere terminates TLS in front of it. Native TLS is not
taken: about forty lines and two flags against twenty-five crates on the
advisory surface and certificate renewal at the operator, for what a proxy
already does. A reference shape, with nothing but the proxy able to reach the
node's port:

```nginx
server {
    listen 443 ssl;
    server_name node.example.org;
    ssl_certificate     /etc/letsencrypt/live/node.example.org/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/node.example.org/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:7001;
        # OVERWRITE, never append: the node is told to trust this header, and
        # a client-supplied hop would then choose its own rate-limit bucket.
        proxy_set_header X-Forwarded-For $remote_addr;
    }
}
```

The node is started with `--trust-forwarded-for`, an assertion about the
deployment: sound only when the proxy overwrites the header and nothing else
can reach the port. Without it the node keys its read budget on the socket's
peer, which behind a proxy is the proxy, so every reader shares one bucket and
the community throttles itself; trusting a header any client can set turns the
budget off. `--bind-all` additionally exposes the transaction-gossip surface
(`/p2p/*`), which authenticates the envelope by a shared `--cluster-token`;
every transaction on it is verified on its own signature at the ingress
regardless, and consensus messages travel on Malachite's own authenticated mesh.

**A validator that is down and comes back.** The WAL is kept
`--prune-margin-blocks` back, one day of blocks by default at the pace an empty
block is held to, and that is how long a node may be down and still rejoin from
a peer. Below every peer's floor: `edet-node malachite export-snapshot --home
DIR --out FILE` on a healthy node and `import-snapshot --home DIR --from FILE`
here. The import checks that the file is a legal state of this chain whose
commitment a certified block claims, and refuses a home already at that
height; where the file came from is the operator's to vouch for. A validator
that merely lost its peers needs only a restart.

**Founding a validator federation** is four steps, nothing hand-written:

1. **Every operator** mints a consensus key: `edet-node malachite keygen --out
   priv_validator_key.json`. The file is 0600, the private half is never
   printed, and the command refuses to overwrite an existing key. Each operator
   sends the author the printed hex and, separately, their **member** key's
   public half, and the printed peer id to the operators who will list them.
   The two keys differ on purpose: the consensus key lives unencrypted on a
   host that answers the internet, the member key signs `Accept`, `Settle` and
   `DeclareSupply`.
2. **The author** writes the genesis: `edet-node genesis init --chain-id ID
   --out genesis.json --validator ADDR:MEMBERKEYHEX:CONSENSUSKEYHEX:POWER ...
   --underwriter ADDR:SUPPLY ...`. It draws a fresh state-root salt, refuses a
   published dev key, refuses fewer than four powered validators and writes
   that floor into the file, which the ledger then holds on every removal,
   seals amounts to the parties unless `--open-amounts`, and refuses a founding
   roll the boundary cannot name. Every operator holds the same file, or they
   compute different roots.
3. **Every operator** writes their home: `edet-node malachite init --home DIR
   --genesis genesis.json --key priv_validator_key.json --listen
   /ip4/0.0.0.0/tcp/26600 --peers /dns4/b.example/tcp/26600/p2p/PEERID,...`.
   It refuses a key the genesis does not name, a member key on the host, a
   name in the listen address, and a peer whose id the genesis does not carry.
   Peers may be names, and each ends in the peer id `keygen` printed for that
   validator: a pair of validators keeps one connection, dialed by the lower
   id, so the node dials only the peers whose id is above its own.
4. **Run the release build** behind TLS: `target/release/edet-node malachite
   --home DIR --client-port 7001`.

`crates/node/tests/malachite_federation.rs` walks all four against real
processes, the only place a non-dev genesis is booted.

**A peer named by DNS is resolved once, at boot**, by the system resolver, and
rewritten to an `/ip4/` or `/ip6/` literal before the consensus transport sees
it, so a DNS change needs a restart. A node refuses to start if peer discovery
is enabled, if a peer is a `/dnsaddr/`, if the listen address names a host, or
if a name does not resolve. That is what keeps the resolver advisory `just
audit` carries unreachable, as a property of the code.

## Status

**Every design question is closed**, including the two that once had no
proposed resolution: priority under a binding ceiling, and whether an uninsured
loss should touch the creditor's own standing. Both are refused and gated. What
stands between this and a real obligation is not a decision: a pilot genesis
with on-device custody exercised end to end, and an external audit.

| | |
|---|---|
| `crates/kernel` | 42 tests + a 2-test cross-pin against an independent scipy reference |
| `crates/state` | 64 + 300 across seventeen suites, each re-auditing all seven invariants over sets after every transition, accepted or refused, and holding the write gate's memo to the definition's verdict |
| `crates/node` | 215 + the engine's eight harnesses: live Malachite clusters over real OS processes, real TCP gossip and the real wire codec, a federation founded from a non-dev genesis, a paused validator rejoining without a restart, a dropped and a late validator dialed back in, and a federation a wide area apart |
| `ui/` | 328 tests across 35 suites, svelte-check clean, production build clean |
| `crates/swarm` | 24 pinned corpus scenes, each auditing every transition, 16 archetype probes and 6 replay probes. Every archetype names, in its test's doc comment, the mutation that turns it red, and all seven in the table `just swarm` carries were applied by hand and reverted |
| `sim/` | an independent max-flow oracle with its own reservation path, 17 security probes, a fixture-freshness gate |
| `paper/` | 54 pages, no undefined references, every measured row naming a probe in the tree |

Zero clippy warnings across the workspace, with and without the engine feature.

Implemented: the capacity model in full; the twenty-transition state machine
(the sale cascade, settlement, transfer, extension, cure, the permissionless
default crank with subrogation, consented arbitration, supply declarations,
guardian rotation, governance, operation bonds and forfeiture, suspension and
exit); atomic re-denomination; the state root with inclusion proofs; the
replication core. Five model extensions are built and measured: loss
substitution, seed amendments, the governance weight, the insured horizon and
the seat slot. Two are refused rather than built: the loss pool (a covenant is
funded by nothing the ledger can hold, and funding a pledge is reserving, which
is the insured tier), and automating the support chain (a drain relieves and
does not rebuild, so the beneficiary's approval is a choice of relief now
against standing later).

**Not production software.** No external audit, no custody of real
obligations. A green `just ci` is a claim about the gates that exist: an
adversarial finding can pass every ledger invariant, several have, and green
says nothing about the client, which `ci` never runs. The reasoning behind each
rule is in the code beside it; the chronology is in `git log`.

## What is left

Nothing that needs a decision; `CLAUDE.md` carries the decisions most likely to
be re-opened by mistake. What remains needs people, devices and an outside
reader.

### Blocking: nothing here may hold a real obligation

**An external security audit.** The invariants are a theorem about capacity,
not about recourse, custody or the boundary, so a defect that respects the
theorem passes them; four did, each found by reading and each closed with its
probe in `git log`. `AUDIT.md` is the brief: the five surfaces the gates do not
quantify over (recourse, custody, the `f64`/integer boundary, governance
weight, the consensus binding), their entry points, what probes them, and the
question worth an outsider's hour.

**An audit of the consensus engine.** Malachite is pinned to commit `bcac2b2`
on `main`, because the tagged `v0.5.0` drops a height's replayed WAL entries;
`no_wal_entry_is_dropped_at_the_start_of_a_height` holds the pin, and
upstream's own suite runs clean on this toolchain at it (43 passed, 10 ignored
by upstream). Its safety argument is upstream's and unexamined here. What this
repo tests is the binding, on loopback: four-validator clusters over real OS
processes, real TCP gossip and the real wire codec, one adversarial pass, a
founding ceremony from a non-dev genesis, a paused validator rejoining, a
state carried between homes, and a partition healing
(`a_partition_heals_when_the_link_returns`). Two choices made here matter to an
auditor. An `Invalid` verdict from the application is a fail-stop rather than
a vote, so the screen answers only the parent-relative half it can and defers
the rest to commit. Discovery is off, and a pair of validators keeps one
connection because only one side dials it, the lower peer id, decided at boot
from the node's own key (`engine_node::split_by_dialer`), which is why every
peer entry names a peer id: measured with the fourth validator paused, ninety
seconds, twice each in one sitting, 69 and 70 blocks on one connection per
pair, 16 and 26 with 11 and 23 streams refused on two. Latency through an
in-process relay per validator (`just latency`), in one sitting: 0.26 s per
block a loopback apart, 0.25 s at 50 ms one way, 0.51 s at 150 ms, and a
federation fifty milliseconds apart is asserted live in `engine-test`.
`CLAUDE.md` carries the mechanism. What remains is a run on validators that
are really apart.

**On-device custody, on real hardware.** The identity vault's device key is
wrapped by an AndroidKeyStore AES-GCM key and the blob lives under
`noBackupFilesDir`, so a restored phone finds a blob it cannot open, fails
closed and routes to the recovery phrase rather than reading as a custody
downgrade. `DeviceKeyStoreTest` drives the round trip, the blob's directory,
a blob whose wrapping key was deleted, an unknown blob version and the
argument check; `.github/workflows/android-custody.yaml` runs it from its own
Gradle root, `src-tauri/android-custody`, with a second job asserting that
regenerating the app project leaves the thirteen tracked files under `gen/` —
which wire in both plugin modules and carry `MainActivity` — untouched. All
seven run green on an arm64 handset running Android 15 (API 35, `just
android-custody-device`), the failure paths among them, and the wrapped blob on
that handset sits under `noBackupFilesDir` at 61 bytes (version, IV, key, GCM
tag), unrewritten since it was created, with the vault opened and signed across
reinstalls. **What no run has covered** is StrongBox, an OS upgrade and a real
restore to a second handset, which is the walk in `just android-run`. Nothing
in `just ci` runs the client application: `just e2e` is a node
script against a real engine, and `clippy` does not reach `src-tauri`, which
only `fmt-check`, `tauri-check` and `audit` name.

**A pilot genesis.** No federation stood up, no seed sized against actual
trade, no default carried through to a cure. The seed is peak simultaneous
insured credit, not annual volume: it is reserved, released and reserved
again, so it turns over once per settlement term, 12× a year at the 30-epoch
floor and 4× on 90-day terms (`just seed-table`, which the paper prints and a
gate holds). `just size-seed` runs a synthetic year against four candidate
seeds, founders seated by trade, newcomers arriving through members with
standing, members leaving, the ceremony on the founder's cadence: a seed of
5,000 seats its 250 rows and refuses the next thirty-nine epochs into the
measured year, 20,000 seats every arrival, and rate headroom does not carry
over between epochs, so a ceremony a month grows the ceiling 27% a year and
only one every epoch compounds at β. A seed is lowered by `DeclareSupply` or
raised by a rate-bounded ceremony, so one set wrong at genesis is not moved
back quickly.

### Known and bounded: real, priced, not blocking

**The seat ceiling binds.** A row is a stock priced by a reservation on the
stake graph that no transition releases: one bond unit of the sponsor's reach
for as long as the row holds anything, given back by the epoch sweep a year
after seating once the row holds, owes and is named by nothing
(`tests/retirement.rs`), and buying the row one self-act of its own, spent
whether or not the write is accepted (`tests/slot.rs`). A community seats
`Σ supply / bond unit` rows and then nobody until a ceremony raises the seed,
375 for a seed of 7,500.00; a member's own reach caps whom they bring in, 25
behind an edge of 500.00; an honest population greeting newcomers seats 113
rows over 120 ticks against a ceiling of 375, meeting it only where one
member's backing has faded. Two smaller costs: a seat is never rerouted for a
later one, so a seat can be refused that a full re-solve would fit, and the arc
under a seat is not floored against decay, so a sponsor whose backing fades
loses the reach without getting the seats back.

**One ledger, and this is the envelope.** Measured on one box (`just cost`),
underwriters at one percent of accounts:

| accounts | state root, full | state root, one block | write gate | write gate, memoised | one reservation | resident |
|---|---|---|---|---|---|---|
| 20,000 | 21.1 ms | 0.031 ms | 3.9 ms | 0.00 ms | 7.0 ms | 61 MB |
| 50,000 | 87.9 ms | 0.075 ms | 25.8 ms | 0.02 ms | 35.7 ms | 145 MB |
| 100,000 | 110.1 ms | 0.099 ms | 38.8 ms | 0.03 ms | 42.5 ms | 298 MB |

An acceptance is the gate plus the reservation: 11 ms at 20,000 and 81 ms at
100,000, both a capacity query. A block is the state root plus the audit; an
ordinary block pays the one-block root column, and the epoch boundary pays
the full build. Both memos rest on the monotonicity the witness above uses: a
lower bound settles a threshold whenever it passes. Carrying substantially more on
one order is open, and splitting does not answer it: severing a stake graph
destroys every edge crossing the cut, and two orders becoming one is a
founding, not a migration.

**The state root is linear in the block, and its privacy has a window.** Each
leaf's salt binds the epoch and each section commits its leaf count, so an
ordinary block rehashes only the rows it wrote and the paths above them.
Measured as the pair in one sitting, the full build against one block through
the incremental tree: 1.1 ms against 0.007 ms at 1,000 rows, 6.1 against 0.009
at 10,000, 30.6 against 0.012 at 50,000, 73.6 against 0.015 at 100,000, with
the working copy each block was applied to gone beside it (0.1, 1.2, 17.6 and
19.0 ms). What a block scales with is the out-degree of the accounts it wrote
to. The disclosure is bounded: inside one epoch a member holding two of their
own proofs learns, from the sibling hashes, whether the record next to theirs
in id order changed between the two heights; across epochs nothing links, and
a proof is served only to its subject or to a validator. The full build is the
definition and the patched tree is what a validator runs, computed side by
side after every transition in the state harness and the swarm.

**Lists are served in pages of at most 500 rows**, by cursor
(`?after=<id>&limit=<n>`, answered with the last id served as `next`), one
read per page at the endpoint's price. The wallet walks up to eight pages and
then says what it left out, which keeps a lying node from starting a walk it
never ends; a counterparty past the last page is reachable by address
(`/whois`).

## License

GPL-3.0-or-later. See `LICENSE`.
