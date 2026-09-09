# edet dev tasks. `just` to list; `just dev` for the local cluster + UI.

_default:
    @just --list

# --- checks ----------------------------------------------------------------

# Workspace tests (kernel + state + replica + replication), then the
# feature-gated code the default workspace build compiles out entirely.
#
# The three lines are NOT redundant, and the second and third are the whole
# point. `--workspace` builds edet-node with no features, so everything behind
# `serve` and `malachite` — including their `#[cfg(test)]` modules — is
# invisible to it: `tests/persistence.rs` is `#![cfg(feature = "serve")]`, and
# 95 of edet-node's 209 unit tests only appear under `malachite`.
#
# That gap is not theoretical: `engine-check` is a `cargo build`, and a build
# never compiles test modules, so a `Block` literal in `engine_codec.rs` or
# `engine_context.rs` can sit uncompilable behind a green build. A build gate
# cannot stand in for a test gate; that is exactly how a broken test file
# stays green.
#
# Each line re-links edet-node under a different feature set, so this is
# slower than one invocation. That cost is inherent to feature-gated code:
# the alternative is a gate that passes without having compiled the code.
#
# `--features malachite` stops at `--lib` on purpose. The malachite integration
# tests drive real OS processes over loopback and contend for ports when the
# runner executes them in parallel, so they stay in `engine-test`, run one
# binary at a time.
#
# `cargo nextest run` rather than `cargo test`: a process per test, so a test
# that aborts names itself instead of taking its binary's whole run with it,
# and the flags these recipes need (`--test-threads`, `--run-ignored`,
# `--no-capture`) sit on the runner rather than behind a `--`. **nextest does
# not run doctests.** The workspace has none — every fence in a doc comment
# here is ```text, which rustdoc does not compile — so nothing is lost, but a
# doctest added later needs a `cargo test --doc --workspace` line beside these.
test:
    cargo nextest run --workspace
    cargo nextest run -p edet-node --features serve
    cargo nextest run -p edet-node --features malachite --lib

# Build the engine binary against the pinned Malachite release (git deps, so
# it's slow). A build, not a test — `engine-test` below is what RUNS it, and
# a green build here proves nothing about behaviour.
engine-check:
    cargo build -p edet-node --features malachite

# **The binary a validator runs.** Every other launcher here builds debug,
# deliberately: they are harnesses, and a harness is worth more when it is
# quick to rebuild and slow to run. A validator is the other way round, and
# `[profile.release]` in the workspace manifest keeps the overflow checks on,
# so what it gains is speed and not silence. The cost figures `just cost`
# prints are release figures and describe this binary.
node-release:
    cargo build --release -p edet-node --features malachite --locked

# The engine's integration tests — real OS processes, real TCP gossip, real
# signatures, real wire codec — RUN, not merely compiled.
#
# These are the tests of the code that actually ships: everything a client can
# claim about COMMITTED state is asserted here, because this is the only
# harness in the tree that commits anything. `just test`'s in-process half
# covers the client API, which needs no consensus.
#
# Run one binary at a time. That is the whole reason these sat outside `ci`:
# cargo runs test BINARIES concurrently, and each of these binds fixed
# loopback consensus ports, so in parallel they fight over them and fail for a
# reason that has nothing to do with the code. Sequential costs ~2 minutes and
# removes the flake entirely. Do not fold these into one `cargo nextest run`
# call: nextest runs tests from every selected binary in one pool, so folding
# them puts the port-colliding harnesses back in parallel with each other.
#
# Do not run with any other node up on this machine. These bind fixed loopback
# ports (consensus 26600+/26800+/27000+/28911+/28921, client
# 7401/7411/7412/7421/7441/7471),
# and 7401 in particular is easy to be holding by accident from a hand-started
# node — a collision there fails `malachite_http` in a way that looks like a
# consensus flake and is not one.
#
# The same is true of anything else on the box: a consensus base of
# 27500, which is `passim`'s port on Fedora, and when that service started
# every multi-node test and `just e2e` began failing at their liveness
# timeouts at once. If that pattern appears — the multi-node tests failing
# together while the solo ones pass — check `ss -ltn` before the diff, and set
# EDET_CONSENSUS_BASE_PORT when generating the testnet.
engine-test:
    cargo nextest run -p edet-node --features malachite --test malachite_app
    cargo nextest run -p edet-node --features malachite --test malachite_cluster
    cargo nextest run -p edet-node --features malachite --test malachite_http
    cargo nextest run -p edet-node --features malachite --test malachite_byzantine
    cargo nextest run -p edet-node --features malachite --test malachite_federation
    cargo nextest run -p edet-node --features malachite --test malachite_recovery --test-threads=1
    cargo nextest run -p edet-node --features malachite --test malachite_swarm --test-threads=1
    cargo nextest run -p edet-node --features malachite --test malachite_latency --test-threads=1

# **The block interval across latencies**: a four-validator testnet whose
# every link runs through an in-process relay delaying each byte by a fixed
# one-way latency, at 0, 50 and 150 ms in one sitting — every other figure in
# the tree is validators a loopback apart, and this is the term they lack.
# A measurement, not an assertion; the ordinary probe beside it asserts only
# liveness at 50 ms and runs in `engine-test`.
latency:
    cargo nextest run -p edet-node --features malachite --test malachite_latency --run-ignored=only --no-capture --test-threads=1

# The REAL networked Malachite cluster test, one harness with its output kept
# — real OS processes, real TCP gossip, real signatures, real wire codec
# (crates/node/tests/malachite_cluster.rs). 4-validator genesis, only 3
# processes started ("tolerating 1 down"), a real signed tx submitted; asserts
# every live node commits and all agree on the state hash. `engine-test` runs
# the same binary in `ci`.
malachite-cluster-test:
    cargo nextest run -p edet-node --features malachite --test malachite_cluster --no-capture

# What happens to a validator that STOPS — the half no other harness touches.
#
# Three of four keep committing while the fourth is paused (quorum is exactly
# three, so that is the whole of "tolerates one down"), the paused one comes
# back on its own or after the restart an operator gives it, and a state is
# carried between homes by `export-snapshot` / `import-snapshot` for the case
# no restart answers: a node below every peer's `--prune-margin-blocks`, where
# value sync has nothing left to serve it.
#
# `--test-threads=1`: both probes drive four OS processes on fixed loopback
# ports, and in parallel they would fight over the machine rather than over
# the ports.
malachite-recovery-test:
    cargo nextest run -p edet-node --features malachite --test malachite_recovery --no-capture --test-threads=1

# Clippy across everything, warnings as errors — including the feature-gated
# code, for the same reason `test` does: `--workspace` builds edet-node with no
# features, so without the second line `serve` and `malachite` are never
# linted. That is not hypothetical either: a 9-argument function and a
# `Default::default()` field assignment are the two lints that hide in the
# engine when no gate looks at it.
#
# `malachite` implies `serve`, so the second line covers both feature sets and
# there is no third.
clippy:
    cargo clippy --workspace --all-targets -- -D warnings
    cargo clippy -p edet-node --features malachite --all-targets -- -D warnings

fmt:
    cargo fmt --all
    cargo fmt --manifest-path src-tauri/Cargo.toml --all

# Both trees. `src-tauri` is excluded from the workspace (Tauri drags in
# webkit/gtk and its own build graph), so `--all` here reaches none of it —
# with one line the client is never formatted at all. rustfmt COMPILES
# NOTHING, so unlike `clippy` or a build it needs
# none of that toolchain and belongs in `ci` on any runner.
fmt-check:
    cargo fmt --all --check
    cargo fmt --manifest-path src-tauri/Cargo.toml --all --check

# Simulation suites + kernel cross-pin.
sim:
    python3 sim/run.py --fixtures

# --- the strategy swarm ---------------------------------------------------

# **The pinned corpus**: a population of behaving agents over the real
# transition function, judged by the real audit, held to the `Summary` each
# scene produced when it was pinned.
#
# In `ci` as its own line even though `test` already runs the binary, because a
# red must NAME its cause: `cargo nextest run --workspace` reports "edet-swarm failed"
# and this reports which entry moved and which field. The second run is a
# cached build plus seconds.
#
# Measured on this box in one sitting, as a RATIO because a wall-clock figure
# is about the machine: the corpus costs **2.6x `just sim`** (30.8 s against
# 11.9 s of `python3 sim/run.py --fixtures`), on a cached build. Twenty-four scenes,
# `Audit::EveryTransition` throughout — so every transition of every run is
# judged by the witness, the cold audit and the cached one in lockstep, which
# is where the cost is and what it is for.
#
# **A scene's SIZE is what this budget is spent on**, and the audit is `O(E)`
# per transition: growing `everything` to 60 seats takes the corpus from 30 s
# to 3m05s, because the sybil farm inside it then seats 874 rows and every one
# of them is in `E`. Size belongs to `swarm-search`, which is out of band.
#
# What a moved pin means is in `crates/swarm/tests/corpus.rs`. It has to be
# READ: a `refused` count that went to zero is a rule that stopped firing.
#
# **What it DETECTS**, each mutation applied by hand once against this corpus
# and reverted:
#
# | mutation to the state machine        | what goes red             | through                     |
# |--------------------------------------|---------------------------|-----------------------------|
# | `accept` skips `ET-CTR-005`          | every entry               | `refused["ET-CTR-005"]` → 0 |
# | `mark_expired` releases the hold     | 11 of 20, as VIOLATIONS   | the audit, on the spot      |
# | `names_a_fresh_key` → `false`        | `sybil-farm`              | `state_root`                |
# | `refund` removed                     | `griefer`                 | the strip billed its signer |
# | a second `Exit` admitted             | `exit-under-suspension`   | `refused["ET-MEM-002"]` → 0 |
# | the award minted at quorum           | `late-defaulter`          | awarded before the close    |
# | `adoption_threshold` keyed on kind   | `coalition-half`          | `enacted` 2 → 3             |
#
# The second row is the one worth reading twice: releasing a defaulter's hold
# does not move a count, it breaks the conservation witness, and the run stops
# at the transition that did it with the seed, the tick and the agent named.
swarm:
    cargo nextest run -p edet-swarm --test corpus --no-capture

# Regenerate every pinned entry from what the tree does now, then READ THE
# DIFF. The `seed-table` / `seed-table-check` shape: one recipe measures, one
# asserts, and the diff between them is the review.
swarm-pin:
    EDET_SWARM_PIN=1 cargo nextest run -p edet-swarm --test corpus --no-capture

# **The search, deliberately out of band**: fresh seeds, long, in parallel
# across seeds with rayon (runs are independent, so one seed is one thread).
#
# A randomised search inside a gate is a gate that goes red for a reason nobody
# can reproduce. What goes in the gate is the corpus; what a search FINDS
# becomes a corpus entry. It exits 1 on the first violated invariant and prints
# the seed, the tick, the agent, the intent, the transaction and the command
# that reproduces it.
#
# `--release` because wall clock is the whole point here, and the workspace
# keeps `overflow-checks` on in release so a run still fails the way a
# validator does.
swarm-search N='256' FROM='1' POP='everything' TICKS='365':
    cargo run --release -p edet-swarm -- search --from {{FROM}} --count {{N}} --population {{POP}} --ticks {{TICKS}}

# Name one violation exactly: the same seed in `Audit::EveryTransition`, which
# is what makes the report reproducible rather than anecdotal.
swarm-replay SEED TICK POP='everything' TICKS='365':
    cargo run -p edet-swarm -- replay --seed {{SEED}} --population {{POP}} --ticks {{TICKS}} --until {{TICK}}

# **Q2, the distributional half.** Every figure is a ratio against a control
# population — the same seeds, the same seats, every treatment seat honest,
# aged the same ticks — because an epoch advance decays every stake and a
# control that has not advanced the same number of epochs is not a control.
#
# Nothing here is pinned and nothing is written into the tree. A Q2 figure
# quoted anywhere names this recipe, its seed range and its population beside
# it, exactly as `just cost` figures name the box.
swarm-q2 N='50' POP='everything' TICKS='365':
    cargo run --release -p edet-swarm -- q2 --seeds {{N}} --population {{POP}} --ticks {{TICKS}}

# The cost tables, measured — every quantity the paper's cost figures are about.
#
# The paper's §Implementation quotes a millisecond figure per
# capacity query at four community sizes, and a cited figure with no probe
# behind it is not a measurement.
#
# The two lines are not redundant, and the second is the one an acceptance-time
# reading misses. The first is the ACCEPTANCE cost: one capacity query, which
# is what `Tx::Accept` runs once through `reserve_capacity`. The second is the
# BLOCK cost: `Replica::commit_block_unchecked` runs the whole invariant audit
# after every committed block, and evaluating invariant 1 over the family costs
# one query per underwriter — `1 + U + 2` FULL queries whatever the block
# contained. That is what the DEFINITION costs, and `audit` still pays it. A
# validator does not: the memo in `invariants.rs` keeps an ordinary block from
# computing anything, and where decay kills the memo the stored hold is a
# WITNESS the audit verifies instead. So the second line measures both arms on
# one state — the definition against what a validator actually runs — and it is
# the second column that bounds community size.
#
# **A block pays for two whole-ledger passes and only one of them is the
# audit.** The state root re-encodes and re-hashes every record on every block,
# with no memo, so its cost is a function of the ledger's SIZE rather than of
# what the block contained — the same shape the memo removed from the audit.
# `a_block_pays_for_the_root_as_well_as_the_audit` measures the two side by
# side, because a community-size budget has to add them.
#
# Deliberately NOT in `ci`: a wall-clock assertion is a flake generator, and on
# a loaded machine this varies by 2x run to run. Run it on an idle box and
# record the hardware beside any figure quoted from it.
#
# `--test-threads=1` is not decoration. The runner executes the tests inside
# one binary in PARALLEL by default — nextest as concurrent processes, cargo
# as threads — so the state harness's two wall-clock probes ran
# concurrently and contended for the same cores — 7.6 s where a serial run reads
# 5.1 s, and a per-set figure that fell as the underwriter count ROSE. A
# measurement harness that races itself is the loaded box it exists to avoid.
cost:
    cargo nextest run --release -p edet-kernel --test cost --run-ignored=only --no-capture --test-threads=1
    cargo nextest run --release -p edet-state --test cost --run-ignored=only --no-capture --test-threads=1

# **The seed-sizing rehearsal** — a synthetic year of trade against four
# candidate seeds, reporting peak simultaneous insured credit, the share of
# rows the community insured, and the rows the write gate refused outright.
#
# The seed is the one number a founding community cannot revise upward without
# a second ceremony, and §Adoption's turnover table gives the rate and not the
# rest of the answer. This is the rehearsal: the process is synthetic, its
# parameters are the founder's own guesses, and what it removes is the class of
# error where those guesses were never carried through the mechanism at all.
#
# Every parameter is an environment variable with a default, listed in the
# module doc of `crates/state/tests/sizing.rs` — the shape of the trade
# (`EDET_SIZE_TRADES_PER_EPOCH`, `EDET_SIZE_MEAN_AMOUNT`, `EDET_SIZE_TERMS`),
# who is in it (`EDET_SIZE_UNDERWRITERS`, `EDET_SIZE_MEMBERS`), how it behaves
# (`EDET_SIZE_SETTLE_ON_TIME`, `EDET_SIZE_CURE_AFTER`) and the candidate seed
# itself (`EDET_SIZE_SEED`). `just` exports the calling environment, so
# `EDET_SIZE_MEMBERS=800 just size-seed` is the whole interface.
#
# `--test-threads=1` and `--release` for the same reasons `cost` has them: the
# binary also carries the two ordinary probes, and a harness that races itself
# measures the contention rather than the ledger.
size-seed:
    cargo nextest run --release -p edet-state --test sizing --run-ignored=only --no-capture --test-threads=1

# The first-contact journey against a REAL node, walked as an ordinary
# member: admission, onboarding (key -> member id), resolving a counterparty's
# address, and a committed trade.
#
# This exists because every other gate in `ci` is blind to a whole class of
# break, and two of them shipped. `dev_genesis` seeds every founder as a
# VALIDATOR, and every harness only ever acted as a founder — so a rule that
# refused ordinary members looked fine everywhere. The rule here is therefore
# that the script must never act as a founder: it admits a real member and
# asserts `is_validator == false` before it proves anything else.
#
# Runs against the ENGINE, so what it proves is true of the binary we ship.
# Starts its own node on a dedicated port and tears it down, so it is
# self-contained.
e2e: ui-install
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build -p edet-node --features malachite
    HOME_DIR=$(mktemp -d)
    trap 'kill $NODE 2>/dev/null || true; rm -rf "$HOME_DIR"' EXIT INT TERM
    # One validator, power 1, so it reaches quorum alone — this gate is about
    # the app-level journey over a real engine, not about BFT agreement, which
    # `engine-test`'s multi-node cluster covers.
    ./target/debug/edet-node malachite testnet --home "$HOME_DIR" --nodes 1 >/dev/null
    ./target/debug/edet-node malachite --home "$HOME_DIR" --index 0 \
      --client-port 7598 --client-peers "http://127.0.0.1:7598" \
      > "$HOME_DIR/node.log" 2>&1 &
    NODE=$!
    # Wait for the chain's EPOCH to stop moving — not for `/health`, and not
    # merely for height > 0.
    #
    # A fresh genesis starts at epoch 0 while the wall clock is ~20,000 epochs
    # ahead, and `MAX_EPOCH_ADVANCE_PER_BLOCK` caps catch-up at 10,000 per
    # block. The first few blocks therefore race the epoch forward in huge
    # jumps, and a transaction minted during that race — with a validity window
    # a few epochs wide — is already expired (`ET-TX-002`) by the time a
    # proposer includes it. The submit succeeds, the commit never arrives, and
    # it reads as a consensus fault when it is really a clock catching up.
    # Measured here: epoch 10,000 at the first block, 20,663 a moment later.
    #
    # This is invisible to any harness that drives its own clock from block
    # height: it is a property of the code we deploy, and only a real engine
    # node shows it.
    #
    # The condition is exact rather than heuristic: wait until the chain's
    # epoch has REACHED the one wall-clock implies. "Looks stable for a moment"
    # is not good enough — catch-up proceeds in one clamped jump per wall-clock
    # SECOND (`begin_block` is idempotent within a second), so
    # the epoch sits still for ~1s at a time while still climbing. Measured:
    # a poll pair 250 ms apart both read 20,000 and declared it settled, while
    # the true target was 20,663 three seconds later.
    #
    # `|| e=""` matters: with `pipefail`, a curl that cannot connect yet fails
    # the pipeline, and `set -e` would abort during the very startup window
    # this loop is here to wait out.
    want=$(( $(date +%s) / 86400 ))
    for i in $(seq 1 240); do
      e=$(curl -sf http://127.0.0.1:7598/network 2>/dev/null \
          | sed -n 's/.*"epoch":\([0-9]*\).*/\1/p') || e=""
      if [ -n "$e" ] && [ "$e" -ge "$want" ]; then break; fi
      sleep 0.25
    done
    node ui/scripts/e2e-first-contact.mjs http://127.0.0.1:7598

# Can the desktop/mobile client's dependency graph still be resolved?
#
# Two seconds, and no GTK/WebKit needed — `cargo metadata` resolves without
# building, which is exactly the failure that happened: adding a TEST crate
# (`malachitebft-test-cli`, needing `toml >= 0.8.21`) to edet-node made
# src-tauri unresolvable against Tauri's GTK stack, which pins
# `toml_datetime = "=0.6.3"`. The client was unbuildable for two days and
# nothing noticed, because `src-tauri` is excluded from the workspace and no
# gate ever touched it.
#
# This does NOT compile it — `tauri-check` does, and is in `ci` too now. This
# stays because it is the cheap half and it is the half that broke: a graph
# conflict is caught in a second, without the webkit/gtk toolchain.
tauri-deps:
    cargo metadata --manifest-path src-tauri/Cargo.toml --format-version 1 > /dev/null

# Known-vulnerability check over both dependency trees.
#
# The client custodies BIP39 phrases and an encrypted seed vault, so "a
# dependency with a published advisory shipped in the wallet" is a finding, not
# a chore. Split deliberately by what actually reaches a user:
#
#   - HIGH and CRITICAL in a RUNTIME dependency fail the build. Those are the
#     ones that ship to a device holding a seed phrase.
#   - MODERATE in a runtime dependency is reported, not failed, and the current
#     set is a deliberate decision rather than an oversight: three `svelte`
#     advisories, all SSR/`bind:innerText` XSS. This client is a Svelte 4 SPA
#     that never server-renders, so none is reachable, and the only fix npm
#     offers is `svelte@5` — a framework major that would land untested on a
#     wallet to close an advisory that does not apply. Revisit when the app
#     moves to Svelte 5 for its own reasons, or if an advisory lands that a
#     client-side app CAN reach.
#   - DEV/build tooling is reported and never fails. A compromised build tool
#     is a genuine supply-chain path to a signed wallet, but it is not
#     something a release branch should block on; it is where the only
#     critical/high in the tree live (`vite`, `vitest`).
#   - `cargo audit` runs when installed and FAILS with exit 2 when it is not —
#     see the last paragraph of this comment, and CLAUDE.md. CI installs it;
#     `nix develop` ships it.
#
# The rule of thumb: this gate must stay green on a clean tree, or it stops
# being read. Anything downgraded here is downgraded WITH ITS REASON, above —
# and a machine missing a checker is not a clean tree.
#
# One override carries its reason here too, because it is the only thing in
# `ui/package.json` that pins a dependency nothing in this repo imports.
# `overrides: { "deepmerge-ts": "^8.0.0" }` closes GHSA-ggr8-5vv4-36mx, which
# arrives through `shepherd.js` (the product tour, a runtime dependency) and
# which no version bump can close: shepherd 14.5.1 and the latest 15.2.3 both
# declare `deepmerge-ts ^7.1.5`. Two things a reader should know, and both are
# in CLAUDE.md under "a green audit is a claim about installed package versions".
# First, the pin is TREE HYGIENE and not a change to the wallet — shepherd
# inlines deepmerge-ts into its own dist, which imports nothing, so the
# production bundle is byte-identical before and after (measured). Second, what
# actually bounds the exposure is reachability: shepherd deep-merges only
# `floatingUIOptions` and the tour options, and every Shepherd option in
# `ui/src/edet/tour.ts` is an in-repo literal with `floatingUIOptions` never
# set. Which is the general caution — this gate reads the dependency TREE, so a
# library that VENDORS its dependencies carries their code straight past it.
#
# And this gate queries a live registry, so it can go red with no commit behind
# it. When `just ci` fails, check whether the failing line is `audit` first —
# and the recipe says WHICH of the two happened rather than
# leaving it to be read off one exit code. `npm audit` asks the bulk advisory
# endpoint and, whenever that request fails for any reason, silently retries the
# legacy `audits/quick` one, which the registry now answers `400` with a notice
# that it is retired; the command then exits non-zero exactly as it does for a
# real advisory. Measured: three consecutive runs of the identical command read
# bulk-200, quick-400, bulk-200. `scripts/npm-audit.py` asks in JSON, retries
# while the answer is not a report, and exits **1** when the dependency tree is
# at or above the floor and **2** when the registry never answered — the second
# being a failure too, because going green on a claim nobody verified is the one
# thing a gate may not do.
#
# Both halves treat a checker that could not RUN the same way, and 2 means the
# same thing in each: the tree is not implicated and nobody verified it either.
# A missing `npm` reaches the python half's 2 on the reasoning above; a missing
# `cargo-audit` reaches this one's. That symmetry is the whole point — the two
# halves of one recipe are the easiest place in a tree for the same question to
# get two answers, since only one of them is ever being edited. **A gate is a
# claim about what it CHECKED, and a machine missing a checker is not a clean
# tree**: a skip that exits 0 makes `just ci` green over a floor nobody read,
# and leaves it to a human to remember to say so. There is deliberately no
# opt-out, because an opt-out is the green resolution the note above refuses.
#
# TWO RUST ADVISORIES ARE CARRIED, both in `hickory-proto` 0.25.2, and both
# with their reason — same policy as the npm side, and the same two questions:
# is it reachable, and can it be fixed here at all. `hickory-proto` arrives
# through `hickory-resolver` <- `libp2p-dns` <- `libp2p` <- Malachite's network
# and discovery crates, which are pinned to a release tag, so no `cargo update`
# reaches a fixed version: 0.25.2 is the newest 0.25.x and the fix lands in
# 0.26.1, a semver-major that `libp2p-dns` 0.44 does not take.
#
#   - RUSTSEC-2026-0118, NSEC3 closest-encloser proof validation loops without
#     bound and allocates until OOM. **Not compiled**: the advisory says it is
#     reachable only "when built with the `dnssec-ring` or `dnssec-aws-lc-rs`
#     feature and configured to perform DNSSEC validation", and this tree
#     enables neither — `hickory-proto` gets `std`, `tokio`, `futures-io` and
#     `hickory-resolver` gets `system-config`, `tokio`. That premise is CHECKED
#     below rather than asserted here, because a proposal's premise is the part
#     nothing gates: if a dnssec feature ever turns on, this recipe fails.
#   - RUSTSEC-2026-0119, O(n^2) name compression while encoding a message,
#     amplifying a CPU-exhaustion DoS. Compiled, and reached only when a name
#     is actually resolved. **No name reaches the transport**, and that is now
#     a property of the CODE rather than of the configurations this repo
#     writes: `EdetApp::load_config` calls `resolve_peer_names`, which rewrites
#     every `/dns*/` persistent peer to an `/ip4/` or `/ip6/` literal through
#     `std::net::ToSocketAddrs` — the system resolver, not hickory — and
#     refuses to boot on discovery, on a `/dnsaddr/` peer, on a name in the
#     listen address, or on a name that does not resolve. The premise is gated
#     by `engine_node::tests::a_dns_peer_is_resolved_at_boot_and_never_dialed_
#     by_name`, whose mutation is the rewrite skipped. The cost is that a DNS
#     change needs a node restart.
#
# `cargo audit --ignore` drops an advisory from its OWN output entirely, so the
# recipe names both out loud before it runs. A carried advisory nobody can see
# is the silent skip again, one level in.
#
# WARNING-KIND ADVISORIES DO NOT FAIL THIS GATE, and that is a decision rather
# than an oversight. `cargo audit` denies vulnerabilities and only
# PRINTS the `unmaintained`/`unsound` kinds — in the workspace today `bincode`
# and `paste` (the latter through libp2p's netlink crates behind Malachite's
# pin), and in the client's tree the gtk-rs GTK3 bindings Tauri's Linux backend
# links. `bincode` is the one this tree could act on, and the action taken is
# to make the migration cheap
# rather than to do it under an advisory: the dependency is named in exactly one
# file (`crates/state/src/codec.rs`), which also writes the byte-level format
# down and pins it with literal vectors, because changing it forks the chain. What this gate
# claims is therefore exactly "no advisory of kind VULNERABILITY, and the two
# carried above". Adding `--deny warnings` would make the gate red on a
# maintainer stepping away from a crate, which is not a fact about this tree and
# not one anybody here can act on. So the printed warnings are the record, and
# they stay visible for the same reason the two carries do.
audit:
    #!/usr/bin/env bash
    set -uo pipefail
    python3 scripts/npm-audit.py --prefix ui --fail-at high
    npm_status=$?
    if [ "$npm_status" -ne 0 ]; then
        exit "$npm_status"
    fi
    echo "--- cargo ---"
    if ! command -v cargo-audit >/dev/null 2>&1; then
        echo "cargo-audit is not installed, so the Rust advisory floor was NOT checked." >&2
        echo "  install: cargo install cargo-audit --locked   (or: nix develop)" >&2
        exit 2
    fi
    # The premise under RUSTSEC-2026-0118's carry, checked rather than
    # asserted: the advisory is reachable only in a build with a dnssec
    # feature, and this one has none. A `cargo tree` that cannot run at all is
    # a 2 for the same reason a missing checker is.
    if ! feats=$(cargo tree -p edet-node --features malachite -e features -i hickory-proto 2>&1); then
        if printf '%s' "$feats" | grep -q 'did not match any packages'; then
            feats=""   # hickory is gone from the tree: nothing to carry
        else
            echo "could not read hickory-proto's enabled features, so the carry below is UNVERIFIED:" >&2
            printf '%s\n' "$feats" >&2
            exit 2
        fi
    fi
    if printf '%s' "$feats" | grep -q 'feature "dnssec'; then
        echo "a dnssec feature is enabled on hickory-proto, so RUSTSEC-2026-0118 is COMPILED IN" >&2
        echo "  the reason it is carried no longer holds — see the comment above this recipe" >&2
        exit 1
    fi
    if [ -z "$feats" ]; then
        echo "hickory-proto is no longer in the tree — the two carries below apply to nothing."
        echo "  drop them from this recipe and from CLAUDE.md."
    else
        echo "carried with their reason (comment above; CLAUDE.md):"
        echo "  RUSTSEC-2026-0118  hickory-proto  NSEC3 validation loop — not compiled (no dnssec feature, checked)"
        echo "  RUSTSEC-2026-0119  hickory-proto  O(n^2) name compression — no fix under libp2p-dns 0.44; no name reaches"
        echo "                                    the transport: load_config resolves DNS peers with the system resolver"
        echo "                                    and refuses discovery and dnsaddr (engine_node::tests::"
        echo "                                    a_dns_peer_is_resolved_at_boot_and_never_dialed_by_name)"
    fi
    cargo audit --ignore RUSTSEC-2026-0118 --ignore RUSTSEC-2026-0119 || exit 1
    # The client is outside the workspace, so the line above never reads its
    # lockfile: `src-tauri/Cargo.lock` is a second dependency tree, and a green
    # here would otherwise be a claim about the node while the wallet carried
    # whatever it carried. No carries on this line: the client has no libp2p
    # and no resolver, so neither hickory premise applies to it.
    echo "--- cargo (client, src-tauri/Cargo.lock) ---"
    cargo audit --file src-tauri/Cargo.lock || exit 1

# Everything CI runs.
#
# `engine-test` and `e2e` are the two lines that run the engine rather than
# merely compiling it. They cost ~3 minutes; the alternative is a green CI
# that has never run the code we deploy.
#
# There is no `cluster-test`: `--features serve --lib` is what `test`'s second
# line already covers in full, and the name would promise a cluster.
ci: fmt-check paper-constants-check seed-table-check proof-fixture-check tx-digest-check view-shape-check clippy test engine-check engine-test sim swarm ui-check ui-test e2e tauri-deps tauri-check audit

# --- paper -----------------------------------------------------------------

paper:
    cd paper && pdflatex -interaction=nonstopmode edet.tex && pdflatex -interaction=nonstopmode edet.tex

# Regenerate paper/sections/generated-constants.tex from the kernel's own
# constants.rs. Run this after touching a genesis value; the paper's prose
# cites the resulting \kConst... macros instead of a hand-typed number, so a
# kernel change and a paper rebuild can never again show two different
# figures for the same constant.
paper-constants:
    python3 scripts/paper-constants.py

# Gate: fails if generated-constants.tex has drifted from constants.rs, i.e.
# someone changed a genesis value and didn't run `paper-constants` (or edited
# the generated file by hand). Pure parse-and-diff, no LaTeX toolchain
# needed, so it belongs near the front of `ci` with the other cheap static
# checks rather than after the slow build/engine/e2e lines.
paper-constants-check:
    python3 scripts/paper-constants.py --check

# Regenerate paper/sections/generated-seed-table.tex by MEASURING it.
#
# §Adoption tells a founding community to size its seed against peak
# simultaneous insured credit rather than annual volume, and prints how much
# trade one seed insures in a year at each settlement term. Those figures were
# hand-typed and produced by nothing — and the fastest term among them was one
# the ledger refuses outright, since a maturity below `min_maturity_epochs` is
# `ET-CTR-MATURITY-TOO-SHORT` and no `ParamKey` reaches that field. A table
# nothing produces is a paragraph with numerals in it.
#
# `crates/state/examples/seed_table.rs` runs the real transition function
# through a year of trade per term and prints the tabular; this splices it in.
seed-table:
    python3 scripts/seed-table.py

# Gate: fails when the paper's seed-turnover table no longer matches what the
# probe measures — a change to decay, to the reservation floor under it, to
# the maturity floor or to `reserve` moves these figures, and the paper is the
# one place that would otherwise go on printing the old ones.
seed-table-check:
    python3 scripts/seed-table.py --check

# --- client cross-pin ------------------------------------------------------

# Regenerate the `RUST_*` inclusion-proof fixtures the client's vitest pins,
# from the normative implementation itself.
proof-fixture:
    python3 scripts/proof-fixture.py

# Gate: fails if the client's pinned proofs no longer match what `root.rs`
# emits.
#
# Without it, a change to `Section::ALL` leaves `ui/src/lib/proof.ts` folding
# every `section_path` at the wrong shape — so the client refuses every genuine
# proof the node serves, while `proof.test.ts` stays green because the fixture
# it pins was generated before the change and its own stale prover verifies it
# perfectly. Implementation and oracle drifting TOGETHER is the one failure a
# cross-pin is supposed to be immune to.
#
# Cheap (one `cargo run` of an example) and static, so it belongs near the
# front of `ci` with the other parse-and-diff checks.
proof-fixture-check:
    python3 scripts/proof-fixture.py --check

# --- client signing digest -------------------------------------------------

# Regenerate the signing-digest vectors the client's vitest pins, from the
# normative implementation itself.
tx-digest-fixture:
    python3 scripts/tx-digest-fixture.py

# Gate: fails when the client's own digest encoder no longer agrees with
# `crates/node/src/block.rs::tx_digest`.
#
# This is the gate that lets a wallet BE a wallet. A client that fetched the
# bytes it is about to sign from whichever node it happens to be reading
# (`/tx/digest`) and signed the reply — or co-signed a pending request by
# signing the `digest` field the same node served, with nothing checking that it
# covers the `tx` the member is looking at — would be signing on that node's
# word. "The embedded node IS the wallet's own code" would justify it, and this
# app embeds no node: it reads one the member CHOOSES, and `custom` is any URL.
# A node can answer with the digest of `Accept { debtor: victim, creditor:
# attacker, amount: 10000 }` and collect a valid signature. `/tx/check` is no
# help: the same node answers it.
#
# Computing the digest on the device means a second implementation of one
# canonical encoding — bincode of a Rust enum, whose variant TAGS are its
# declaration order — and two implementations drift. So the vectors are
# generated by the normative one, one per variant of the alphabet, and the
# example's own exhaustive `match` fails to compile until a new transaction has
# one. Cheap and static apart from a `cargo run`, so it belongs at the front of
# `ci` with the other cross-pins.
#
# Measured, mutating `ui/src/lib/txdigest.ts` against this fixture: a wrong
# variant tag, a big-endian `u64` and a dropped chain-id length prefix each turn
# it red. A fourth mutation — dropping the sort that makes an arbitration panel
# a SET — does NOT, because `serde_json` emits the fixture's `BTreeSet` already
# sorted; `tx-digest.test.ts` carries its own probe for that one.
tx-digest-check:
    python3 scripts/tx-digest-fixture.py --check

# --- client view shapes ----------------------------------------------------

# Report which fields the client's read-view types declare that no node serves.
view-shape:
    python3 scripts/view-shape.py

# Gate: fails when `ui/src/lib/api.ts` declares a view field
# `crates/node/src/serve/views.rs` does not serve.
#
# The third gate of the same family, and the one whose absence cost most. A
# client type is not a claim about what the node serves, and nothing read both
# sides: `/proposals` served a headcount the ledger never used, six admission
# fields were declared and served by nobody (one of them silently disabling
# onboarding, because a field that reads `undefined` disables whatever is gated
# on it), and a `NetworkView` carrying a governor this model does not run — with the network page
# rendering a "Community brake" at `NaN%` under help text promising a mechanism
# this design explicitly refuses.
#
# Cheap and static apart from one `cargo run` of an example, so it belongs at
# the front of `ci` with `paper-constants-check` and `proof-fixture-check`.
view-shape-check:
    python3 scripts/view-shape.py --check

# --- ui --------------------------------------------------------------------

ui-install:
    cd ui && npm install

ui-check: ui-install
    cd ui && npm run check

ui-build: ui-install
    cd ui && npm run build

ui-test: ui-install
    cd ui && npm test

# --- tauri desktop/mobile client -------------------------------------------
# These wrap `nix develop .#tauri` so the webkit/gtk toolchain is present.

# Run the desktop client against a node started beside it.
#
# The client embeds no node. One that did would silently found a private
# single-validator chain on published dev keys at every first run —
# irreversible, and a dead end, because standing cannot be carried between
# ledgers. So this starts
# a real `edet-node` on 7001 and points the client at it; the client's own
# first-run wizard asks which network, and `local` is this one.
#
# Onboard by restoring a founder phrase (printed below) — a fresh solo chain
# has no other device to trade with yet.
tauri:
    #!/usr/bin/env bash
    set -euo pipefail
    HOME_DIR=$(mktemp -d /tmp/edet-tauri-node.XXXXXX)
    cargo build -q -p edet-node --features malachite
    trap 'kill $NODE 2>/dev/null || true; rm -rf "$HOME_DIR"' EXIT INT TERM
    ./target/debug/edet-node malachite testnet --home "$HOME_DIR" --nodes 1 >/dev/null
    ./target/debug/edet-node malachite --home "$HOME_DIR" --index 0 --client-port 7001 &
    NODE=$!
    echo "node on http://127.0.0.1:7001 — pick \"a node you run yourself\" in the wizard"
    echo "founder recovery phrases:"
    cargo run -q -p edet-node -- dev-phrases --members 5 | sed 's/^/  /'
    nix develop .#tauri --command bash -c "cd src-tauri && cargo tauri dev"

# Build the desktop bundle.
tauri-build:
    nix develop .#tauri --command bash -c "cd src-tauri && cargo tauri build"

# Compile the Tauri backend (no bundling). In `ci`, because nothing else
# builds the client: `src-tauri` is excluded from the workspace,
# so `clippy` and `test` reach none of it and `tauri-deps` only resolves the
# graph. A crate no gate compiles is a crate that breaks quietly.
#
# Two routes to the same compile, never a skip. `nix develop .#tauri` carries
# webkit/gtk on this machine; a runner that has them from its package manager
# builds directly. If neither can, this FAILS — an exit 0 without a build
# would be the lie the rest of this file exists to avoid.
tauri-check:
    #!/usr/bin/env bash
    set -euo pipefail
    if command -v nix >/dev/null 2>&1; then
        nix develop .#tauri --command bash -c "cd src-tauri && cargo build --locked"
    else
        echo "no nix — building against the system webkit/gtk"
        cd src-tauri && cargo build --locked
    fi

# There is no `cluster-tauri` recipe: N desktop windows, each embedding its
# own node and peering with the others, is something the client cannot do,
# because it runs no consensus. What such a recipe is really for is `just
# dev N`: N real validators, and one UI bound to each.

# --- android ---------------------------------------------------------------
# The mobile client on a real device. `nix develop .#android` carries the
# SDK/NDK and a Gradle-compatible JDK; `cargo tauri` exists only inside it.
#
# These recipes are where the incantation lives, so that `just --list` shows
# the mobile client beside the desktop one — for a client whose whole custody
# story is the OS keychain.

# Build the debug APK for ONE ABI (default `aarch64`, which is what every
# arm64-v8a device runs).
#
# `cargo tauri android build` defaults to all four ABIs and the other three
# are dead weight on a device you are holding. The frontend needs no separate
# step: `beforeBuildCommand` in `tauri.conf.json` runs `npm --prefix ui run
# build` first.
#
# Debug rather than release, because `gen/android` has no release signing
# configured, so debug-signed is the installable build. It is ~450 MB, which
# is what `[profile.dev] strip = "debuginfo"` in `src-tauri/Cargo.toml` got it
# down to from 892 MB — at which size the USB link on this machine dropped
# mid-transfer, and `adb push` reported success while nothing landed.
android-apk ABI='aarch64':
    nix develop .#android --command bash -c "cd src-tauri && cargo tauri android build --debug --apk --target {{ABI}}"

# Install the built APK on the connected device.
#
# **It refuses an APK older than the sources**, and that is the whole reason
# this is a recipe rather than a line in the README. An APK left on disk by an
# earlier device session walks the onboarding of whatever protocol the tree
# implemented then, on a build that looks entirely healthy. A stale artefact
# tests the wrong thing and says nothing while it does.
android-install:
    #!/usr/bin/env bash
    set -euo pipefail
    APK=src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
    if [ ! -f "$APK" ]; then
        echo "no APK at $APK — run \`just android-apk\` first" >&2
        exit 1
    fi
    STALE=$(find crates ui/src ui/package.json src-tauri/src src-tauri/Cargo.toml Cargo.lock \
        src-tauri/capabilities \
        src-tauri/gen/android/edet-keystore/src src-tauri/gen/android/edet-keystore/build.gradle.kts \
        src-tauri/gen/android/edet-background/src src-tauri/gen/android/edet-background/build.gradle.kts \
        src-tauri/gen/android/app/src/main/java src-tauri/gen/android/app/build.gradle.kts \
        src-tauri/gen/android/settings.gradle \
        -newer "$APK" -print -quit 2>/dev/null || true)
    # Every tracked source the APK is built from: `edet-keystore/src` covers
    # both `main` and `androidTest`, `edet-background/src` the service and its
    # merged manifest, and `app/src/main/java` the one file of the app module
    # that is ours — `MainActivity.kt`, which is what re-resumes the WebView.
    if [ -n "$STALE" ]; then
        echo "the APK is older than the tree — $STALE has changed since it was built" >&2
        echo "  it was built $(date -r "$APK" '+%Y-%m-%d %H:%M')" >&2
        echo "  run \`just android-apk\` before installing, or this tests the code you had then" >&2
        exit 1
    fi
    adb install -r "$APK"

# Build, install, point the phone at a node on THIS machine, and follow the log.
#
# **`adb reverse` is what makes the phone's `local` network work.** The app
# embeds no node; its `local` entry is `http://localhost:7001`, and on a
# handset `localhost` is the handset. `adb reverse tcp:7001 tcp:7001` makes
# the phone's own 7001 come out of the USB cable and land on this machine's
# 7001, so the node started below is what the app reads — no LAN address, no
# firewall, nothing to configure in the app.
#
# The reverse is re-established on every run because it does not survive an
# unplug, and a dropped cable otherwise looks exactly like a broken node.
#
# Two logs matter, and the default `-s` filter hides the second: Rust
# panics arrive under `RustStdoutStderr` (Tauri mobile redirects stderr
# there), and anything the UI says — including the vault's own warnings —
# arrives under `Tauri/Console`, because that is where the WebView's console
# goes (`RustWebChromeClient.kt`). A filter that omits it is silent through
# every client-side failure.
#
# Build, install, start a node, reverse :7001 onto the phone, launch, tail.
# The walk, in order, on the handset this installs to:
#   1. first run: choose a network, create the keys, seal the vault;
#   2. kill the app from the task switcher; reopen; the keys come back from
#      the vault without the recovery phrase;
#   3. sign a transaction against the node behind `adb reverse`, watch it
#      commit;
#   4. back the device up and restore it (or transfer to a second handset):
#      the app must show the recovery-phrase copy, never a broken key store
#      and never a fresh key minted over the dead blob;
#   5. restore from the phrase; sign and commit again.
#
# **Background mode is measured here and nowhere else.** `just ci` gates the
# arming rule, the cadence and the notification (`ui/src/lib/background.ts`,
# `background.test.ts`); whether the WebView keeps running once Android pauses
# it is a claim about a handset. The series, all of it against the node this
# recipe starts, with a purchase opened from
# `../edet-lan/tools/newcomer-sim.mjs` and timed to the signature:
#
#   A/B. **The subject is SURVIVAL, not latency**, and a probe that opens the
#      purchase first measures the wrong thing: backgrounded with the screen
#      ON, an UNARMED app signed in 3.0 s, and with the screen off in 14.5 s —
#      `onPause` does not stop the JavaScript at that horizon, so a short
#      control separates nothing. Background the app, turn the screen off,
#      leave it alone for N minutes, and open the purchase THEN.
#
#      Measured that way at 10 minutes, both arms in one sitting:
#        control (background mode off)  NOT SIGNED within 180 s
#        armed                          SIGNED after 35.1 s
#      The process survived in BOTH — the pid was identical throughout — so
#      what stops is the WebView's JavaScript and not the process, which is
#      what `MainActivity`'s re-resume addresses and why the service alone
#      would not have been enough. 35.1 s against a `BACKGROUND_INTERVAL_MS`
#      of 10 s also says the platform throttles a resumed-but-hidden page
#      beyond our interval: 10 s is what this client asks for, not a rate it
#      gets. Confirm the service with `adb shell dumpsys activity services
#      org.edet.client` — `isForeground=true types=0x40000000` is
#      FOREGROUND_SERVICE_TYPE_SPECIAL_USE.
#   C. **Doze**, which nothing here fights:
#        adb shell dumpsys deviceidle force-idle   # network suspended
#        adb shell dumpsys deviceidle unforce      # and back
#      A phone on charge never enters it; the copy beside the switch says so.
#   D. **Battery**, the two cadences A/B in one sitting (never against a figure
#      from another day):
#        adb shell dumpsys batterystats --reset
#        # ... an hour armed ...
#        adb shell dumpsys batterystats org.edet.client | grep -i 'uid.*edet'
#   E. **A passphrase set, then `adb shell am kill org.edet.client`**: the rule
#      must not go silent in silence — the last thing the member sees is the
#      "Unlock to keep deciding" notification, and after the kill nothing signs
#      until they unlock.
#
# **A chain whose members all score at the cold-start ceiling cannot measure
# this**, and that is the first thing to check rather than the last: a founding
# underwriter's capacity is zero by construction, so the rule HOLDS every
# counterparty at the default thresholds and the walk reads a hold instead of a
# signature. Raise `accept` on the device for the probe and restore it after —
# the subject is whether the page keeps running, and the score is not what is
# being measured. Check the buyer can post the work bond too: a member with no
# write headroom gets `ET-BND-001` from `/tx/check`, and the engine correctly
# refuses to sign what the ledger would reject.
android-run ABI='aarch64': (android-apk ABI) android-install
    #!/usr/bin/env bash
    set -euo pipefail
    HOME_DIR=$(mktemp -d /tmp/edet-android-node.XXXXXX)
    cargo build -q -p edet-node --features malachite
    trap 'kill $NODE 2>/dev/null || true; adb reverse --remove tcp:7001 2>/dev/null || true; rm -rf "$HOME_DIR"' EXIT INT TERM
    ./target/debug/edet-node malachite testnet --home "$HOME_DIR" --nodes 1 >/dev/null
    ./target/debug/edet-node malachite --home "$HOME_DIR" --index 0 --client-port 7001 \
      > "$HOME_DIR/node.log" 2>&1 &
    NODE=$!
    until curl -sf --max-time 1 http://127.0.0.1:7001/health >/dev/null 2>&1; do sleep 0.2; done
    adb reverse tcp:7001 tcp:7001
    echo "node on :7001, reachable from the phone as http://localhost:7001"
    echo "founder recovery phrases (restore one to act as a member):"
    cargo run -q -p edet-node -- dev-phrases --members 5 | sed 's/^/  /'
    adb logcat -c
    adb shell monkey -p org.edet.client -c android.intent.category.LAUNCHER 1 >/dev/null
    echo "following logcat — Ctrl-C to stop (that also stops the node)"
    adb logcat -s RustStdoutStderr:V AndroidRuntime:E Tauri/Console:V

# --- android custody, on an emulator ---------------------------------------
#
# **This is the only thing in the tree that RUNS Android custody.** The device
# key is wrapped by an AndroidKeyStore AES-GCM key and the blob is written
# under `noBackupFilesDir`; both exist on a device or an emulator and nowhere
# else, so every claim about that path was reasoning until these recipes.
#
# `nix develop .#android` carries the emulator and one x86_64 API 34
# `google_apis` image, and this box exposes `/dev/kvm` — without KVM the
# emulator runs under full emulation and a `connectedAndroidTest` takes tens of
# minutes rather than one.
#
# **The instrumented test has not been run anywhere yet.** The run that gates
# custody is the CI one (`.github/workflows/android-custody.yaml`, on GitHub's
# own image), and these two recipes are the same run for an operator whose
# emulator starts. Do not read a green `just ci` as covering any of this:
# neither recipe is in it.
#
# The test runs from `src-tauri/android-custody`, not from the app project:
# the app project's settings apply `tauri.settings.gradle`, which the `tauri`
# crate's build script writes during an Android build of the app, so a
# checkout that has not cross-built the app cannot configure it. That root
# holds the module and the one tauri module it compiles against, located by
# `cargo metadata`, and it is what CI runs too.

# Create the AVD if it is missing, start it headless, and wait for boot.
#
# `-no-snapshot` so every run starts from the same device state: a snapshot
# carries the Keystore and the app's files forward, and this test's whole
# subject is what happens to those across a wipe.
android-emulator:
    #!/usr/bin/env bash
    set -euo pipefail
    nix develop .#android --command bash -c '
        set -euo pipefail
        IMAGE="system-images;android-34;google_apis;x86_64"
        export ANDROID_AVD_HOME="''${ANDROID_AVD_HOME:-$HOME/.config/.android/avd}"
        mkdir -p "$ANDROID_AVD_HOME"
        if ! avdmanager list avd -c | grep -qx edet; then
            echo no | avdmanager create avd -n edet -k "$IMAGE" --force
        fi
        if adb devices | grep -q emulator; then
            echo "an emulator is already running"
        else
            emulator -avd edet -no-window -no-snapshot -no-audio -gpu swiftshader_indirect \
                > /tmp/edet-emulator.log 2>&1 &
            echo "starting the emulator (log: /tmp/edet-emulator.log)"
        fi
        adb wait-for-device
        # `wait-for-device` returns as soon as adb can talk to it, which is
        # long before the system is up. `sys.boot_completed` is the fact.
        until [ "$(adb shell getprop sys.boot_completed 2>/dev/null | tr -d $'"'"'\r'"'"')" = 1 ]; do sleep 2; done
        adb shell input keyevent 82 || true
        echo "emulator booted"
    '

# The instrumented custody test, on that emulator.
#
# Two halves, and the second is the one no unit test can reach. The gradle run
# drives `DeviceKeyStore` directly — round trip, the blob's directory, and a
# blob whose wrapping key has been deleted failing CLOSED. The adb half drives
# the same store across the two events a member actually meets: a process kill,
# and `pm clear`, which is what a factory reset looks like to an app.
# The instrumented custody test on a HANDSET, which is the run an emulator
# cannot be: a hardware-backed Keystore (StrongBox where the device has it),
# a real `noBackupFilesDir`, and the OS the member actually holds. Refuses
# unless exactly one device is attached, so a result is never a result about
# whichever device `adb` picked. The manual walk after it — keys created, the
# vault sealed, the app killed, the keys recovered, a transaction signed and
# committed, then the same after a restore, where the app must show the
# recovery copy and not a downgrade — is `android-run`.
android-custody-device:
    #!/usr/bin/env bash
    set -euo pipefail
    DEVICES=$(adb devices | awk 'NR>1 && $2=="device" {print $1}')
    COUNT=$(printf '%s\n' "$DEVICES" | grep -c . || true)
    if [ "$COUNT" -ne 1 ]; then
        echo "android-custody-device: exactly one attached device is required, found $COUNT" >&2
        exit 2
    fi
    echo "running the instrumented custody test on $DEVICES"
    nix develop .#android --command bash -c '
        set -euo pipefail
        cd src-tauri/android-custody
        ../gen/android/gradlew --no-daemon :edet-keystore:connectedDebugAndroidTest
    '
    echo "custody round trip passed on $DEVICES; the report is under src-tauri/gen/android/edet-keystore/build/reports/androidTests/connected/"

android-keystore-test: android-emulator
    #!/usr/bin/env bash
    set -euo pipefail
    nix develop .#android --command bash -c '
        set -euo pipefail
        cd src-tauri/android-custody
        ../gen/android/gradlew --no-daemon :edet-keystore:connectedDebugAndroidTest
    '
    echo "instrumented custody test passed on the emulator"

# --- clusters (continued) ---------------------------------------------------

# Left over from when the app embedded a node: `status.json` in the app's own
# data directory carried the committed height. The client runs no node now, so
# there is nothing on the device to ask — the node's height comes from the node
# (`curl $NODE/head`), and what the DEVICE is doing is in `just android-run`'s
# logcat and the app's own Network status screen.

# --- clusters --------------------------------------------------------------

# Peer URL list for a cluster of N nodes on ports 7001..7000+N.
_peers N:
    @seq 0 $(({{N}}-1)) | awk '{printf "%shttp://127.0.0.1:%d", (NR>1?",":""), 7001+$1}'

# One consensus, everywhere: real OS processes, real loopback TCP gossip, real
# Ed25519 commit certificates. Headless is `malachite-cluster`; with UIs it is
# `dev`.

# Launch N (default 4) loopback edet-node Malachite validators as separate
# OS processes, peered over 127.0.0.1, seed one signed transaction, then
# print each node's committed height + state hash every couple seconds until
# Ctrl-C (all healthy nodes' hashes should read identically once they've all
# applied the seeded tx).
malachite-cluster N='4':
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build -p edet-node --features malachite
    BIN=target/debug/edet-node
    HOME_DIR=$(mktemp -d /tmp/edet-malachite-cluster.XXXXXX)
    "$BIN" malachite testnet --home "$HOME_DIR" --nodes {{N}}
    "$BIN" malachite seed-tx --home "$HOME_DIR" --nodes {{N}} --debtor 0 --creditor 1 --amount 10
    pids=()
    cleanup() { kill "${pids[@]}" 2>/dev/null || true; }
    trap cleanup EXIT INT TERM
    for i in $(seq 0 $(({{N}}-1))); do
      "$BIN" malachite --home "$HOME_DIR" --index "$i" > "$HOME_DIR/$i.log" 2>&1 &
      pids+=($!)
    done
    echo "{{N}} Malachite validators on loopback, home $HOME_DIR. Ctrl-C to stop."
    echo "logs: $HOME_DIR/<i>.log"
    while true; do
      sleep 2
      line=""
      for i in $(seq 0 $(({{N}}-1))); do
        f="$HOME_DIR/$i/status.json"
        if [ -f "$f" ]; then
          h=$(python3 -c "import json;d=json.load(open('$f'));print(d['height'])" 2>/dev/null || echo '?')
          s=$(python3 -c "import json;d=json.load(open('$f'));print(d['state_hash'][:12])" 2>/dev/null || echo '?')
          line="$line  node$i height=$h hash=$s"
        else
          line="$line  node$i (no commit yet)"
        fi
      done
      echo "$line"
    done

# Local development on the ENGINE PRODUCTION RUNS: N (default 2) Malachite
# validators, each ALSO serving the browser-client HTTP API the UI already
# speaks (--client-port), plus one UI dev server per node. Consensus runs over
# Malachite's own libp2p mesh; --client-peers is the separate transaction
# gossip that mesh does not carry, so a tx submitted on any device can be
# proposed by whichever node's turn it is.
#
# There is one consensus in this tree and this runs it. Developing against a
# different consensus than you ship is how a rule that holds only on the
# development driver reaches a release — it happened twice on this branch
# before the second implementation was deleted, and the fresh-chain epoch trap
# (the founding ceremony) is invisible to such a driver by construction.
#
# Quorum is ceil(2N/3) — with the default N=2 BOTH nodes must stay up for
# anything to commit. Every instance starts blank: restore a founder phrase
# (printed below). Fresh ledger each run (temp home). Ctrl-C stops all.
dev N='2':
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build -p edet-node --features malachite
    ( cd ui && npm install --silent )
    BIN=target/debug/edet-node
    HOME_DIR=$(mktemp -d /tmp/edet-malachite-dev.XXXXXX)
    "$BIN" malachite testnet --home "$HOME_DIR" --nodes {{N}}
    PEERS=$(seq 0 $(({{N}}-1)) | awk '{printf "%shttp://127.0.0.1:%d", (NR>1?",":""), 7301+$1}')
    CORS=$(seq 0 $(({{N}}-1)) | awk '{printf "%s%d", (NR>1?",":""), 5173+$1}')
    pids=()
    cleanup() { kill "${pids[@]}" 2>/dev/null || true; }
    trap cleanup EXIT INT TERM
    for i in $(seq 0 $(({{N}}-1))); do
      "$BIN" malachite --home "$HOME_DIR" --index "$i" \
        --client-port $((7301+i)) --client-peers "$PEERS" --cors-port "$CORS" \
        > "$HOME_DIR/$i.log" 2>&1 &
      pids+=($!)
    done
    for i in $(seq 0 $(({{N}}-1))); do
      ( cd ui && EDET_NODE="http://127.0.0.1:$((7301+i))" EDET_CHAIN="edet-dev" \
        npx vite --port $((5173+i)) --strictPort > "/tmp/edet-malachite-ui-$i.log" 2>&1 ) &
      pids+=($!)
    done
    echo "{{N}} devices on the Malachite engine:"
    for i in $(seq 0 $(({{N}}-1))); do
      echo "  device $i  →  http://localhost:$((5173+i))   (node :$((7301+i)))"
    done
    echo "founder recovery phrases (restore one per window):"
    "$BIN" dev-phrases --members {{N}} | sed 's/^/  /'
    echo "node logs $HOME_DIR/<i>.log · ui logs /tmp/edet-malachite-ui-*.log"
    wait

# The browser-UI-over-HTTP tests for a Malachite node (crates/node/tests/
# malachite_http.rs): reads/submit over HTTP against engine-committed state,
# the client router's shape, and the tx-gossip hop. Like malachite-cluster-test
# these are real OS processes on fixed loopback ports, so don't run them with a
# cluster already up. `engine-test` runs the same binary in `ci`; this is the
# one-harness form with the output kept.
malachite-http-test:
    cargo nextest run -p edet-node --features malachite --test malachite_http --no-capture

# The pre-vote authentication screen under a REAL misbehaving proposer
# (crates/node/tests/malachite_byzantine.rs): node 0 runs --allow-unsigned and
# proposes a forged transaction to its honest peer over live consensus.
# Asserts it never commits, the cluster keeps making progress, and — the part
# that fails without the screen — the honest validator never signs quorum
# certificates for two different values at one height. Slow (it waits out
# real consensus rounds); `engine-test` runs the same binary in `ci`, and this
# is the one-harness form with the output kept.
malachite-byzantine-test:
    cargo nextest run -p edet-node --features malachite --test malachite_byzantine --no-capture
