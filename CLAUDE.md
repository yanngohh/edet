# CLAUDE.md

`paper/edet.pdf` is the specification; `README.md` is the front door. This file
is what neither states: what the code has already learned, so a change does not
pay for it twice. Every line generalises a defect this tree carried; the case is
in the commit that closed it.

## The model, in one sentence

**Capacity is the maximum flow into an account from the community's
underwriters, across directed stakes of what creditors have placed. Nobody can
stake beyond what they may confer. Outstanding credit reserves its flow.** No
membership status, no attestation, no admission or creation transition: a row is
seated by the first bonded trade that names its key. Every supply arc is seated
by a ceremony, genesis or a `SeedAmendment`, so the underwriter roll IS the
external seed, and `DeclareSupply` can only ever lower one.

Do not re-litigate it. The cut bound, the decay and supply floors, the
substitution split at the set quantifier, the seed base, the free-key write
floor, `Transfer`'s creditor rule and the sweep of `MarkExpired` have not moved
under any probe. What broke, every time, was something at an EDGE.

## Verifying

- `just ci` is the gate and GitHub Actions runs it and nothing else. **Read its
  exit code** (`set -o pipefail`; `just ci | tail` reports `tail`'s status).
- Green is a claim about the gates that EXIST. Seven readers check what no
  compiler does: `paper-constants-check`, `seed-table-check`,
  `proof-fixture-check`, `tx-digest-check`, `view-shape-check`, the client's
  inline English against `en.json`, and `scripts/npm-audit.py`. Prose is not
  gated and cannot be.
- **A validator is a RELEASE build** (`just node-release`) with
  `overflow-checks` kept on: a deterministic fail-stop, never a silent wrap.
  Every other launcher is a debug harness on purpose.
- **`src-tauri` is outside the workspace**, so `--all` does not reach it.
  `fmt-check`, `tauri-check` and `audit` (which reads `src-tauri/Cargo.lock`
  as a second tree) name the client.
  `tauri-check` needs webkit/gtk (`nix develop .#tauri` here, apt on the
  runner). Nothing in `ci` RUNS the client: that is `just android-run` on a
  device. Android custody has its own workflow
  (`.github/workflows/android-custody.yaml`) and `just android-custody-device`;
  a green `ci` says nothing about it, and the nix emulator segfaults here.
  **Background mode's Android half is in the same class**: `ui-test` gates the
  arming rule, the cadence and the notification, and whether a WebView keeps
  running behind a foreground service is a claim about a handset. MEASURED on an
  Android 15 handset, both arms in one sitting, backgrounded with the screen off and
  left ten minutes before the purchase was opened: unarmed NOT SIGNED within
  180 s, armed SIGNED after 35.1 s. **The subject is SURVIVAL, not latency** —
  opened immediately the unarmed app signs in 3.0 s screen-on and 14.5 s
  screen-off, so a short control separates nothing. **The process survives in
  both arms** (identical pid): what stops is the WebView's JavaScript, which is
  why the service alone would not have been enough and `MainActivity`'s
  re-resume is the load-bearing half. 35.1 s against a 10 s interval says the
  platform throttles a resumed-but-hidden page beyond it: the constant is what
  the client ASKS for. The walk, and the two things that make a chain unable to
  measure it, are in `just android-run`'s comment.
- **`ci` costs about ten minutes of engine tests**, eight harnesses over
  loopback. **A wait fails on two different sentences** (`common::Budget`):
  `stalled` when no live node's height moved for 45 s once any had committed,
  `slow` when the target is unmet at the ceiling while heights still move. A
  loaded box produces the second and never the first; under a single budget
  the trio stall reproduced idle 3 times in 9, invisible as anything but
  "timed out".
- Two dev-box hazards: a build-cache daemon that GCs `target/` mid-run
  ("could not execute process … (never executed)"), and every engine harness
  binding FIXED loopback ports (26600, 26800, 27000, 27200–27400 and
  27600–27680 for consensus, 27700–27815 for the latency relays, 7401–7441
  for clients), so a hand-started node or a leaked child fails them. Check
  `ss -ltn` before the diff. **`cargo nextest run -p edet-node` rebuilds
  `target/debug/edet-node`, which a running engine harness spawns from**: never
  build the node while one runs; `cargo check` is safe. A passing engine probe
  deletes its home.
- **A measurement on this box is a RATIO, never a figure.** The same probe
  reads 81.7 ms busy and 25.2 ms quiet: A/B in one sitting, never against a
  number recorded another day.

## Method

- Measure the sentence a claim rests on, not only the figure: a proposal's
  premise, a question's premise and a description of the tree are claims
  nothing gates.
- Test at the quantifier the theorem uses. Every defect here was a bound
  asserted over a set and tested over a singleton, or over one round amount.
- Write the probe that FAILS without the check, and mutate the code to prove
  the probe bites.
- Ask WHICH PARTY and WHICH QUANTITY before believing a reading. A reading one
  party over looks right in every screenshot.
- When you have built a fix, attack it. Several of the sharpest findings came
  out of the fix before them.
- **Silence is not health.** A part the reassembly buffer refuses is logged
  with its reason; the stall's only earlier evidence was a log of votes that
  read healthy. Ask what a log filter would show if this crashed now.

## Settled — re-opened by mistake, not because they are unclear

- **One ledger, one total order.** A community is a region of the graph.
  There is no merge (two orders becoming one choose a validator set and earn
  every stake again from nothing), so **splitting for scale is not
  available**: severing a stake graph destroys every edge crossing the cut.
  The ledger's size limit is an OPEN problem in the paper.
- **An obligation stores its `Held`.** Partial discharge releases the whole
  and re-takes the remainder; a subrogated claim's re-take SHRINKS where it
  is (`loss::shrink`), or a loss pinned to an underwriter moves.
- **`Transfer` needs the creditor only when it drops the insurance**; the
  cascade applies the same test as a bound on the amount, not a signature.
- **One supply map, the external seed, never RAISED at any capacity.**
  Written at genesis and `seed::enact`, lowered by `DeclareSupply`. A raise
  capped by the declarer's capacity is hollow in aggregate: twelve joiners
  behind a seed of 100 declare 204,900 and borrow 204,800 of ledger-labelled
  INSURED credit. "A coalition cannot underwrite itself" is about the
  declarers, not the sybils they back.
- **`bond_headroom` reads `seed_reach`**, not `conferrable` (a promise on
  promises doubles the write channel per accomplice) and not capacity (a
  founding underwriter's is zero).
- **A validator's consensus key is not its member key**
  (`Member.consensus_key`, `Tx::SetConsensusKey`): a host compromise must not
  be an economic identity compromise, nor a wallet restore a hot consensus
  key. Power without a registered key is refused (`ET-VAL-004`); the key
  file is 0600. **A consensus key is a claimed key** (`key_is_claimed`,
  `add_underwriter`, an audit clause), so the separation holds both ways.
- **Changing who ORDERS the ledger takes two thirds of the seed**, everything
  else a half (`adoption_threshold`); suspension is in the higher class when
  its target holds voting power, read off state, not the proposal kind. A
  parameter is recoverable through its door; a validator set is not.
- **Governance weight is a share of the external seed**, every kind. A cut
  says who the seed reached, not who put it up.
- **The wallet computes the digest it signs** (`ui/src/lib/txdigest.ts`,
  `tx-digest-check`). A node-supplied payload is the node choosing what you
  sign, and `/tx/check` is no defence: the same node answers it. The chain id
  comes from the network declaration; for `custom` it is the one the member
  typed beside the URL (`customChainId`), the node's reported id is checked
  against it and never substituted, and with none declared the wallet refuses
  to sign (`SigningRefused('undeclared')`).
- **The client is a WALLET; validators are headless institutional nodes.** A
  handset is a poor validator, and an embedded node founding a private chain
  on published dev keys is a dead end: standing cannot be carried between
  ledgers. The app signs locally and reads a node the member CHOSE
  (`ui/src/lib/networks.ts`), asked before the key exists; a lying node can
  withhold and omit, never forge, and `views::head` makes nodes comparable.
- **A row is seated by the first bonded trade.** Creation cannot be billed,
  and an unbillable transition can only be bounded ledger-wide, a censorship
  lever. `bonded_party` returning `None` is a REFUSAL.
- **A key cannot be CHARGED for a pending-pool entry** (`pending_sign`,
  `PendingPool::awaits`, `pending::invited_by`): keys are free, so the pool's
  occupancy is paid by a member. A signer with no account may co-sign an
  entry a member opened naming that key, or OPEN its own first purchase on
  the seller's INVITATION (`pending::Invite`: the seller's signature over
  chain, key, expiry and nonce, minted beside the "pay me" QR, `edet://pay`),
  which charges the inviter's bucket, admits purchases FROM the inviter only,
  names exactly the two of them, and holds at most `MAX_INVITED_PER_MEMBER`
  open per inviter; a cap per named member WITHOUT a signature lets anyone
  who knows an id keep that member's inbox full of junk. Without an
  invitation, or when the pool refuses, the same signed envelope travels as
  a code (`lib/offer.ts`, `edet://buy`) the seller's wallet verifies over the
  digest it computes itself, co-signs and submits to `/tx`. Buying is the
  one trade the wallet records, for a member and a key alike; the
  seller-side "I sold" form was this rule wearing a UX costume. A keyed
  viewer resolves an address off the anonymous `/members` page (500 rows) or
  names the seller by the key the pay QR carries; `whois` answers a member
  only.
- **The free allowance goes to accounts with something to lose**
  (`bond::free_remaining`); a non-Active member has no budget
  (`ET-BND-005`); the bill goes to the FIRST signer in canonical order who
  can pay, allowance before bond, since two lower bounds cannot rank two
  signers. **A seated row holds ONE self-act its seat paid for**
  (`Member.seat_slot`, `Due::Slot`): spent by the row's own key on a
  transition about that row and nobody else (`bond::self_act_subject`), at the
  gate or at a refusal (`price_refusal`), before any co-signer is billed and
  only where the row has no allowance of its own. A stock bounded by seats: a
  farm of N rows gets N, and a newcomer registers guardians once alone.
- **An underwriter's failure is bounded in AMOUNT by the supply and in TIME by
  the insured horizon** (`ParamKey::InsuredHorizon`, 365 at genesis, in
  (30, 10,000)), measured from `Contract.accepted_epoch`, which a transfer, a
  routed successor and a subrogated piece INHERIT. Measured from now, a chain
  of extensions rolls for ever; `created_epoch` cannot serve, because the
  arbitration window and the retention sweep read it. Read at `book`,
  `extend` and every debtor swap (`within_insured_horizon`), NEVER in an
  invariant. Committed flow is released by repayment, or by the creditor's
  consent to the uninsured tier (a transfer to an uninsured successor, an
  acceptance or an extension past the horizon), never by time and never by
  the underwriter. The sentence that claimed underwriters consent to a long
  maturity named a consent no transition asks for.
- **There is no layer under the uninsured tier.** Funding a pledge is
  reserving, which is the insured tier.
- **A drain relieves and does not rebuild**, so the beneficiary's approval is
  not automatable away: a routed discharge is a debtor swap and writes no
  stake.
- **Priority under a binding ceiling is first-come**; a ceiling rations
  INSURANCE in whole rows.
- **A ring of DEFAULTS is netted by its minimum at the boundary, no
  signature**: everyone is already due, so nobody is paid early; drop that
  condition and it is an early payment. Substitution breaks a ring of insured
  defaults, so the sweep mostly finds uninsured ones.
- **An arbitration award is the median over the whole WINDOW, minted at its
  close**; minting at quorum makes the panel a race. **A quorum below a
  majority is a minority lever**: the median is over ATTESTATIONS, two
  colluders plus one honest arbiter on a panel of sixteen with a quorum of
  three mint the ceiling, and nothing can compel an attestation, so the panel
  AND the quorum are what both parties consent to.
- **An uninsured loss does not touch the creditor's standing**: a debtor
  needs none, so it would be a free key's zero-cost weapon.
- **Every amount in state is a `u64` of minor units; the wire is `f64`.**
  `Params` keeps `v_base`, `dust` and the rates as `f64`, converted once where
  the ledger reads them. `ArbTermsWire` sits beside `ArbTerms`, converted once
  at acceptance past the refusals `to_minor` cannot make; the query layer is
  doubled the same way (`*_minor` is what the ledger calls).
- **The state root is LINEAR IN THE BLOCK; the leaf salt binds the EPOCH.**
  Seven sections; a leaf commits its INDEX and a section its COUNT outside
  the leaf (or leaf 0 of 3 folds to a 4-leaf claim); a block rehashes what it
  wrote and the paths above it, and scales with the OUT-DEGREE it wrote to.
  Measured 1.1 → 0.007 ms at 1,000 rows, 73.6 → 0.015 at 100,000. Disclosure
  is one epoch wide. `state_root` is the definition, `RootCache::refresh` what
  a validator runs, held equal by the driver after every transition.
  Soundness is structural: `journal::Journal` marks every point key and the
  whole map on any `&mut` it cannot attribute, five `raw_mut` sites, all in
  `state.rs`, beside their `touch`es. Leaves hash with an ORDERED collect.
- **The consensus encoding is named in ONE file** (`crates/state/src/codec.rs`),
  pinned with vectors; the wallet reimplements it. A change reaching one call
  site forks the chain while `tx-digest-check` stays green.
- **A ROW IS A STOCK, PRICED BY A RESERVATION ON THE GRAPH.** A seat holds one
  bond unit of flow from the seed to the SPONSOR on a second pair
  (`seat_reserved`/`seat_committed`) capacity never reads, and **only the
  retirement of an EMPTY row releases it**: `release_seat` has two callers, a
  seating that then refused and the sweep (`apply::retire_empty_rows`,
  `ROW_RETENTION_EPOCHS` after seating). Empty is a property nobody can
  impose on another member, so the rule is not a lever, and a farm's emptied
  rows return at most the seats they held
  (`a_farm_cannot_recycle_seats_faster_than_the_retention_window`); without
  retirement a community at its ceiling with a fifth leaving a year holds 41%
  live rows in year four. **A row that sponsors a live seat is named and never
  empty**: invariant 7 asks a sponsor to be a member, and the sweep retiring a
  quiet sponsor under a row it seated halted every node at that boundary, a
  year in, with no attacker. Shared and permanent for the row's life, or a
  farm behind one edge of 500.00 compounds (91 → 1,120 rows in six epochs by
  wash) and a seat that decays is renewed by an accomplice.
  `unit × seats(S) ≤ Σ peak(e)` into any underwriter-free superset; time is
  nowhere in it. Measured: 25 rows behind 500.00, 250 behind 5,000.00; a
  community seats `Σ supply / unit` and then nobody until a ceremony; the
  corpus farm 499 → 23 rows. Cost 29.7 → 35.5 ms at 20,000 accounts. **Three
  things bend, and the paper says so**: the seat is the one bond no transition
  returns; the ceiling binds; `reserve` does not reroute earlier seats. **A
  trade may seat TWO rows** (`fresh_keys` is a count in `0..=2`), both
  reservations before either row (`seat_pair`), and the gate asks the SAME
  SEQUENCE of holds (`can_seat_minor`): one flow of two units admits what two
  holds cannot fit. **`ET-BND-006` arms nothing and charges nothing**; gate
  order: status, work bond (`ET-BND-001`, which does arm), seat. **Two
  readings, both needed**: `capacity`/`reserve` exclude a target's own supply
  (right for credit); `capacity_with_own_supply` includes it (a founding
  underwriter must seat the first members). **Invariant 7 claims conservation
  only**: `seat_reserved ≤ edges` is FALSE by design after decay; a hold is a
  WHOLE unit because `rescale_seat_held` divides at the ROUTE level; a seat's
  SIZE is compared to nothing, since seats taken under different units
  coexist, and a clause against the live price halted the chain on the first
  downward `BondFraction`, the one dial that moves the seat count.
- **A hold SHRINKS where it is and is never re-solved** (`loss::shrink`, live
  and expired rows alike, from `apply::rehold`). Re-solving a live hold cost
  every validator a network build per partial payment, 2.3 ms at 10,000
  accounts against a microsecond for a free transition, and could move it
  onto a preferred underwriter. **A partial payment is at least
  `original / MAX_INSTALLMENTS`** unless it closes the row; "bounded by the
  amount" was `2^51` free durable writes for one allowance slot.
- **One permissionless list** (`bond::is_permissionless`;
  `block::is_permissionless` delegates). A refused crank forgets its id; a
  refused SIGNED free class spends one allowance slot of the first payer who
  has one or is forgotten (`apply::price_refusal`); the ingress keeps an
  envelope naming an unissued id out of the mempool (`apply::names_issued_ids`),
  deliberately not an `apply` rule. `Assent` refuses a member already on the
  record (`ET-GOV-008`); `RotateVeto` deletes the request.
- **A routed claim inherits `min(its maturity, the sale's)`** and `Assumed` is
  keyed by `(creditor, maturity)`. Booked at the sale's date it was the
  re-dating `move_debtor` refuses, on two other members' signatures, insured
  throughout so no default ever fired.
- **Stake is the peak over an obligation's CUMULATIVE repayment**
  (`original − outstanding`), never the installment: two halves of 1,000 stake
  1,000, as the paper's definition says.
- **`ArbTerms` carry the parties and the amount they bind.** Substitution
  keeps the panel on the row it rewrites, the award runs between the ORIGINAL
  parties bounded by the original amount, and `retire_empty_rows` names them.
- **A suspended underwriter may lower and leave**, and the sanctioned may
  propose and assent `Unsuspend` and nothing else (`apply::may_govern`): the
  electorate must survive its own sanctions.
- **A creditor's guardians may sign a discharge** (`signed_or_guardians`,
  mirrored in `authorises`); a debtor's never.
- **`Params::min_validators` is genesis data** (`genesis init` writes 4 for a
  real chain, 1 for the dev one) and every removal path holds it. A real chain
  founds with `seal_amounts` 1.0 unless `--open-amounts`.
- **The bond unit must survive at both doors**: a `BondFraction` or a
  re-denomination under which `to_minor(bond_fraction × v_base) == 0` is
  refused when proposed and when enacted (`unit_survives`).
- **`/tx` returns the outcome hash and the wallet polls it**
  (`watchOutcome`); `tx_rate_key` is canonical order, dedup runs before the
  token, and `/tx` and the pending endpoints sit under the per-IP limiter
  priced by body size.
- **A settlement's stake is capped on the residual the payment LEAVES.**
  `settle`, `cure` and `cascade::discharge_hop` run `rehold` before
  `discharge_credit`; read before the release, a creditor whose backing arcs
  the obligation's own reservation ran through conferred 0.00 for an honoured
  loan (`a_settlement_stakes_against_the_residual_the_payment_leaves`).

## Watched — deliberate, with a cost

- **The electorate is a listed set, so guardian recovery is a governance
  path**: a threshold of an underwriter's guardians seizes a vote too.
- **A creditor who will not sign a discharge calls the insurance, and the
  ledger cannot see it** (`tests/refusal.rs`). A debtor has no unilateral act:
  `Settle` and `Cure` need the creditor, and the sale that nets a debt is a
  purchase THEY record. A refused offer leaves the state root BIT-IDENTICAL —
  `ET-MEM-003` is refunded and its id forgotten — so no rule can be keyed on
  "a discharge was offered", and silence-discharges hands every debtor a
  wait-out. Measured over six debtors against a control aged identically: the
  refuser's capacity, conferrable, seed reach, headroom, allowance and default
  record are IDENTICAL to the signer's, and it ends holding the whole amount as
  a claim on the underwriter where the signer holds nothing. A shared supply
  costs it capacity and never the write surface, which is read gross. Bounded
  by Σ supply, and each use burns a backed debtor — a second claim on the same
  debtor is uninsured, so refusing it substitutes nobody. A deductible does not
  deter (the refuser still nets `(1-d) x amount`) and breaks the promise for
  honest creditors: do not re-propose it. What the debtor has is `Transfer`
  before maturity, and — insured only — the underwriter as a creditor who
  signs the cure.
- **`Transfer` moves WHICH member burns, never WHETHER one does** — decided,
  and not a defect (`tests/handover.rs`). A debtor hands a claim to a consenting
  backed accomplice, the accomplice defaults, and the first debtor carries no
  `open_default` and a full allowance. The accomplice SIGNED
  (`require_signed(new_debtor)`) and pays with capacity the community conferred
  on them by being repaid, so nothing is laundered AWAY: measured over the pair
  on every quantity the ledger enforces — the cut, the capacities consumed, the
  allowance left, `(1, 32)` in both arms — a handover and a plain default are
  identical, and the community pays once either way. What moves is the
  per-member reading, because the handover releases the first debtor's
  reservation where a default keeps it committed, and capacity is what
  `ui/src/lib/risk.ts` scores; no bound is stated over one member, and the
  client shows the ledger's own figure beside its own. Nothing of the
  CREDITOR's moves: same amount, date, acceptance and insured flag, a successor
  the community cannot carry still needs their signature, and the panel stays
  on the original row, still minting between the ORIGINAL parties.
- **A suspended member's supply stays in the assent denominator**; removing it
  makes suspension a franchise act. The sanctioned vote on reinstatement alone,
  which closed the deadlock without moving anybody's weight.
- **`Exit` is terminal and the two status machines do not talk.** A suspended
  member can leave for free, beyond `Unsuspend`, once any supply is lowered to
  zero.
- **A peer id follows the consensus key**, so rotating one is a config edit
  on every peer that lists the validator, and `get_address` resolves the
  node's own key against the GENESIS, so a validator seated after genesis or
  one that rotated does not boot at the node level. Both pre-date the dialer
  rule; the rule adds the config edit.
- **A green `audit` is a claim about installed versions** read off both
  lockfiles' TREES: a vendoring library passes carrying whatever it vendored.
  Two Rust advisories are carried with their reason in the recipe, both
  premises GATED: the dnssec features by a `cargo tree`, and "no name reaches
  the transport" by `resolve_peer_names`, which rewrites every `/dns*/` peer
  through `std::net::ToSocketAddrs` at `load_config` and refuses discovery,
  `/dnsaddr/`, a name in the listen address and a name that does not resolve.
  It denies VULNERABILITIES and prints `unmaintained`/`unsound`: decided, do
  not re-propose `--deny warnings`. **`bincode` 1.3.3 stays**: decided; do
  not re-propose an in-tree codec or `wincode`.

## Rules

- **THE TREE IS A SNAPSHOT, NOT A CHANGELOG.** No dates, review labels, commit
  hashes, "used to", "since", `f5_something()` names. Keep the argument, drop
  the chronology, present tense with its numbers. Exempt: this file and git.
- **A gate is a claim about what it CHECKED.** A machine missing a checker is
  not a clean tree; a skip that exits 0 is a lie; a carried advisory is
  VISIBLE with its reason GATED; a gate that goes red for two unrelated
  reasons separates them in the exit status and is never resolved by going
  green. A suite that asserts a gate PASSES says nothing about what it
  DETECTS.
- **A stock is priced by a reservation on the graph never released; a
  per-account price is a rate.** Per member a farm child and an honest
  newcomer read the same; only a reading over the SET separates them.
- **A write budget bounds a RATE; what a write buys may be a STOCK.** Seating
  is bonded, never allowance-covered; closed rows are retired: a settled,
  transferred or cured obligation after `CLOSED_RETENTION_EPOCHS`, a proposal
  after `PROPOSAL_RETENTION_EPOCHS`, an empty account row after
  `ROW_RETENTION_EPOCHS`, an `Expired` obligation NEVER (a default is live).
- **The establishment floor is a fraction of `v_base` on the SEED's reach**
  (`bond::ESTABLISHED_FRACTION`), not a dust threshold on `conferrable`: dust
  qualifies a wash trade. A fixture at a small denomination sets `v_base`.
- **The WAL is pruned below the last snapshot** with one interval of margin,
  and a node that pruned reports `history_min_height`; an unpruned log grows
  ~31.5M frames a year.
- **A zero-priced transition must destroy its own precondition.** Ask what
  refuses the SECOND call, and its REFUSAL is a write somebody pays for, or
  nothing is kept of it (`price_refusal`).
- **An audit clause that compares a stored fact against a LIVE parameter is a
  halt waiting for the dial**, and the sweep must never retire what another
  row names. Both halts here were one line each.
- **A bond is charged per transaction and says nothing about payload size**;
  a transition that writes a LIST needs its own bound. Ask what else the
  constant you move was guarding.
- **A replay id is a write, so the gate comes first.** Of any durable
  structure: which bond priced this?
- **A claim's MEASUREMENT must exist, and measure the QUANTITY claimed.** A
  wall-clock figure without a generator and a machine is not a measurement;
  vary the other axis; a cost claim needs the RATE of the thing compared to;
  measure against a counterfactual, not a before; `--test-threads=1` where a
  harness would race itself.
- **One code path, two economic acts.** List every transition that reaches the
  line you changed.
- **A limit and a floor are the same arithmetic answering opposite
  questions.** Netting is the point of a limit and double-counting in a
  measure of what a member has to lose.
- **Every discharge is authorised by the party who loses if it is wrong**, or
  by the guardians they appointed to act when they cannot, or bounded so they
  cannot lose. A stake is only placed by a transition the creditor signed.
- **An invariant that passes is not a claim about what it does not count.**
  All six were green while a pool released a defaulter: what dies is recourse.
- **The `f64`/integer boundary is crossed exactly twice**, a payload entering
  `apply` and a view leaving `serve`; a third crossing is a defect. A
  tolerance in an invariant claims the two sides may differ; between a cache
  and the book they may not (an `f64` cache drifted 4e-5 at institutional
  size). An insured obligation owes exactly what it holds.
- **When code and paper disagree, find which is coherent before changing
  either.** A comment is the one part of the tree no gate reads.
- **When a fix turns a test red, read what that test was GUARDING.**
- **When you delete a symbol, grep for what PROSE would call it.**
- **Ledger state carries what must be ENFORCED.** Either something consumes a
  field or it goes; check which.
- **A theorem is stated over the definition it needs.** `Cap` is the RESIDUAL
  cut; the paper carries `Cap⁰` and `drawn(S)`, which
  `invariants::capacity_invariants` checks.
- **What the ledger KEEPS is checkable against `State` and `Member`**: read
  the struct before believing the sentence. Capacity is the one quantity NOT
  kept.
- **A row only appears beside a write somebody paid for AND that succeeded.**
  `dispatch` has no rollback: resolve, validate, seat LAST.
- **An envelope that authorises nothing is refunded and re-appliable** and must
  never reach a block: `edet_state::authorises` at ingress, pre-vote screen
  and commit. `MAX_TXS_PER_BLOCK` == the proposer's batch.
- **A fail-stop is only as deep as the WAL ORDER.** Append LAST, after the
  audit; audit the snapshot at `open` and every replayed block; end the loop
  on a violated invariant; a refusal sets `Replica::halted` and every read
  answers 503.
- **A layer funded by signatures cannot be repaired by a cap.**
- **Ask whether the mechanism you hand a decision to can run when the decision
  is due**: a wallet policy is not a standing consent. **A member's own
  price is shown BESIDE the ledger's score, never in place of it**
  (`ui/src/lib/pricing.ts`).
- **A call that never RETURNS is not an ERROR, and a `catch` cannot see one.**
  Every "on failure" fallback is unreachable until the call is BOUNDED.
- **`setup` runs on the main thread and the WebView is ALREADY LOADING**: an
  IPC call issued into a blocked main thread is never answered.
- **The acceptance rule signs in the WEBVIEW**, so "does it keep deciding" is
  "does that page keep running", and each platform stops it differently.
  Android's `WryActivity.onPause` pauses the WebView (generated by wry on every
  build, never ours), so background mode re-resumes it from `MainActivity` and
  holds a `specialUse` foreground service to keep the process off the cached
  list; the desktop turns the window's close into a hide behind a tray, since
  closing the window ends the process. `specialUse` and not `dataSync`, which
  is cut off after six hours in any twenty-four. The service is
  `START_NOT_STICKY` and CANNOT SIGN: restarted with no WebView it would be a
  notification claiming a rule that is not running, which is also why
  `onTaskRemoved` stops it. **Doze is documented, not fought**: an unplugged,
  stationary device with the screen off has its network suspended for every app
  outside the battery-optimisation allowlist, foreground service or not, so the
  client ACQUIRES no wake lock and deep-links the OS screen where the exemption
  is the member's to grant. (`WAKE_LOCK` and `RECEIVE_BOOT_COMPLETED` ARE in
  the merged manifest: `tauri-plugin-notification` declares them for scheduled
  notifications, which this client does not use. A permission declared and a
  permission used are different claims, and only the second is ours.) Nothing reaches past the process: a killed app, a
  locked vault and a device that is off all sign nothing, and the copy beside
  the switch says so.
- **A default the store persists on SUBSCRIPTION is not a choice a member
  made.** Gate on a flag the STEP writes.
- **The front door is read before the specification, and a manifest is one**:
  README, every version, every `--help`, the tour and onboarding copy. A
  missing string is polish; a wrong one is a promise.
- **A client type is not a claim about what the node serves**
  (`view-shape-check`); a view describing a mechanism the chain does not run
  is read as a promise.
- **An `Invalid` verdict from the application is a FAIL-STOP, not a vote.**
  Upstream's `decide` asserts the value was screened valid, so a node that
  answers Invalid to a block a quorum decides loses its consensus actor and
  keeps running without one. `screen` answers only the parent-relative half
  it can, at `replica.height + 1`, and defers the rest to commit.
- **Discovery is off so no name reaches the transport, and Malachite is
  pinned to a `main` commit (`bcac2b2`), not the tagged release.** The tag
  stamps a height's replayed inputs with the previous height and the WAL drops
  them (`no_wal_entry_is_dropped_at_the_start_of_a_height` gates the pin; at
  the tag it read `Failed to send Append command to WAL actor: channel is
  closed`, ractor's word for a dropped reply); the commit re-dials a
  configured peer itself on a one-second timer, from the configured literal,
  never from an address a peer announced, and keeps an inbound connection for
  the life of the process. **Two connections per pair reorder gossipsub**:
  both sides dial at boot, a message goes to whichever connection is ready, a
  proposal's parts arrive with the close before the hash, and one Nil prevote
  fails a round of three. So **only one side dials: a validator dials the
  peers whose id is greater than its own** (`engine_node::split_by_dialer`,
  at `load_config`, from its own key), which is why every peer entry ends in
  `/p2p/<peer id>`, `keygen` prints one and `init` checks it against the
  genesis; the second connection never exists. A cap on connections per peer
  is NOT this: it closes the newer connection on each side, the sides need
  not agree which, and a connection closed at identify time can carry
  gossipsub's one-time subscription announcement (a two-validator cluster then
  hears and never speaks: `NoPeersSubscribedToTopic`, twenty rounds at
  height 1). A vendored copy of upstream's discovery crate with a lower-id
  tie-break and a 3 s grace did the same and was replaced by the rule: a fork
  nobody upstream reviews, against twenty lines in the node. A/B, fourth
  validator paused, 90 s: 69/70 blocks on one connection per pair, 16/26 on
  two. Below the history floor the remedy is `export-snapshot` /
  `import-snapshot`.
- **A deterministic in-block order keyed on submitter bytes AUCTIONS the
  proposer's lever, it does not remove it**: digest order lets every member
  grind an envelope to the front, and a key nobody knows in advance is the
  proposer's, who assembles the batch. The contention paragraph in §Security
  stands as written.
- **A cross-check whose peer list comes from the party being checked is
  circular.** A network is a SET of URLs declared where the node cannot reach
  it.
- **Ed25519 is DETERMINISTIC, so a signature-keyed replay cache needs a nonce
  in the signed bytes** (`viewer_auth_message`, `edet-view-v2`, 16 random
  bytes; `serve::replay` admits a verified signature once). A bearer token is
  still replayable for its TTL.
- **The client's ORIGIN is part of the wire**: wry serves from
  `tauri://localhost`, `http://tauri.localhost` and `https://tauri.localhost`;
  all three are in the allowlist, or one platform silently reads nothing.
- **`env(safe-area-inset-bottom)` is 0 on Android and always will be**: the
  WebView maps the safe area from the DISPLAY CUTOUT, never from the
  navigation bar, so an edge-to-edge page draws its own footer under the
  buttons (measured: top 37 px, bottom 0, on a screen whose nav bar is 48).
  `WindowInsetsCompat` is the only source, the page PULLS it
  (`BackgroundPlugin.systemInsets`) because a push from `onWebViewCreate`
  lands in the document the real page replaces, and the layout takes the
  `max()` of it and `env()` so no platform is double-counted. Read it with
  **`getInsetsIgnoringVisibility`**: `getInsets` counts only sources visible
  at that instant, and a nav bar hidden behind the shade or mid-animation
  answers 0 truthfully. A window with no insets yet is an ERROR, not a zero.
  A child sized `100dvh` inside the padded frame spends the reserved room
  again, and the frame is `border-box` or its padding is added to the height.
- **Every notice goes through one visibility rule** (`notifyIfAway`): with the
  app in front, what it would say is already on a screen the member can see.
  The payment notice carries `largeBody`, or Android truncates it to one line
  with nothing to expand — the plugin sets `BigTextStyle` on that field alone.
  A request is announced only once it has survived a SECOND poll
  (`lib/waiting.ts`), because the acceptance rule sweeps the same store update:
  announce on sight and a member is told a purchase needs them and then that it
  was signed for them. A support approval raises NO pool entry, so its RISE is
  the event and the first reading is a baseline. **Two channels, because they
  are two interruptions**: a receipt for what the rule signed is frequent and
  never urgent, something waiting on a PERSON is neither, and Android hands the
  importance of a channel to the member — on one channel the frequent kind is
  what they would silence, taking the other with it. `edet-payments` and
  `edet-waiting` are declared at every start (an existing id updates its NAME,
  so they follow the app's language); the service's `edet-acceptance` is the
  Kotlin side's, and belongs to the process rather than to anything that
  happened.
- **MDC caches a slider's track rectangle**, and SMUI's `Slider` asks the
  Svelte context for `SMUI:addLayoutListener` to be told to look again — a
  context only `Dialog` and `List` provide, so a slider on an ordinary page
  had nobody to hear it and its thumb sat at the old width's position after a
  rotation. `App.svelte` provides it at the root, for the screen turning AND
  for the drawer taking its column, which resizes every page under it with no
  `resize` event behind it.
- **Below the expanded breakpoint the drawer covers the page**, and does not
  squeeze it: 256 px out of 408 leaves a column the wallet's own address wraps
  five times in. A section closes it there and only the menu button does above
  it.

## Easy to forget

- **`capacity_of` is net of the credit standing on the member; `conferrable`
  net of the creditor's own borrowing. The write gate is GROSS of live credit;
  only `capacity`, `conferrable` and `reserve` read the residual.**
- **A binding ceiling withholds insurance, not trade**; `reserve` is
  all-or-nothing.
- **A default does not release the flow it committed**, nor substitution nor
  time; `insured` is decided at acceptance and can only FALL.
- **Selling earns no standing.** `discharge_credit` fires in exactly four
  places, `settle`, `cure`, `cascade::discharge_hop` under `net_mutual` and
  `net_rings`, never on a debtor swap; the cascade cannot rescue a defaulted
  claim; a `Sale` is a debtor swap.
- **A `Held` is a flow, not a list of numbers.**
- **`to_minor` CLAMPS AT BOTH ENDS**, and the top halts a chain: `1e300` is
  `u64::MAX` minor units. `State::amount_representable` is asked BEFORE
  conversion at every ingress that books what it names; `settle`, `cure` and
  `declare_supply` convert first and are bounded by the comparison that
  follows (`outstanding`, the current supply), which a clamp cannot pass.
  `MAX_AMOUNT_MINOR = 2^51 − 1` is where the round trip stops being the
  identity, an INGRESS bound that must not enter `invariants.rs`; the audit
  sums in `u128`; `apply::debt_after` before the first WRITE.
- **Zero is absorbing.** `to_minor(x) == 0` is a REFUSAL; `dust_minor()` can
  be ZERO after a downward re-denomination.
- **An empty block is paced to one a second; a block with transactions never
  is** (`engine_malachite::propose_delay`); unpaced, 1.87M heights in 26
  minutes solo.
- **An epoch is a protocol constant** (`unix_secs / EPOCH_SECS`); a fresh chain
  climbs 10,000 epochs per block until it catches up. It is ABSOLUTE — epoch N
  begins at `N × EPOCH_SECS` — so the client shows the DAY rather than the
  number wherever the epoch is absolute (`ui/src/lib/epoch.ts`), taking the
  length from `/network`'s `epoch_secs` and never from a constant of its own.
  Durations in epochs stay durations: a maturity is typed and read back in the
  same unit, and converting one end of that form and not the other is worse
  than leaving it alone. **Advancing an epoch
  DECAYS every stake**: measure against a control aged identically. Integer
  decay drops at least one minor unit an epoch below 43, so an edge of 500.00
  is gone in 346 epochs.
- **`min_maturity_epochs` is 30 and no `ParamKey` reaches it**: the seed turns
  over at most 12× a year; decay costs that nothing (`seed-table-check`).
- **β is per-epoch headroom that does NOT carry over** (`seed::remaining`
  resets at the boundary): a ceremony a month grows the seed 27% a year and
  only a ceremony every epoch compounds at β. **The seat ceiling and the
  credit ceiling are one number**, `Σ supply / unit` rows, so a founding
  membership of M needs a seed of M units; `just size-seed` prints both, and
  `ParamKey::BondFraction` in (0.001, 0.10) is the door that moves the unit.
- **Both memos are LOWER BOUNDS, and a bound never refuses.** The write gate's
  cut is keyed on `(epoch, params, underwriters)` and a bound that fails is
  recomputed exactly BEFORE the verdict (111 ms → 0.02 ms at 100,000). The
  audit's verified cut holds until something DECREASES, and an insured
  obligation stores a feasible flow that invariant 4's floor keeps feasible
  after decay, a WITNESS the audit verifies instead of a cut it recomputes;
  `audit` is the definition and `audit_with_cache` must agree on every state.
  An underwriter seat is a stock the bond schedule prices as a rate; the
  witness moved that cost off the validator.
- **Neither suspension nor exit stops a member CONFERRING**; `Exit` succeeds
  at most once and `Suspended` may call it.
- **An address derives from the key, never the member id.**
- **Genesis is a ceremony, not a verification**; a hand-copied cross-pin
  oracle pins whatever was last pasted (`tests/adversarial.rs` holds the proof
  vectors by hand; `just proof-fixture` regenerates only the client's side).
- **`ContractStatus` is lowercase on the wire.**
- **`src-tauri/gen/` is gitignored and THIRTEEN files in it are force-added**
  (`settings.gradle` wires `:edet-keystore` and `:edet-background` in;
  `tauri.settings.gradle` is written by the `tauri` crate's build script, never
  by `init`, which is why the custody test has its own root
  `src-tauri/android-custody`). Java target 17 across all three modules — the
  app consumes both libraries, and a consumer at a lower target cannot read a
  library at a higher one. **`app/src/main/AndroidManifest.xml` is NOT among
  them**, so a `<service>` or a permission written there is one regeneration
  from gone with every gate green: `edet-background` declares its own in the
  LIBRARY manifest, which AGP merges into the app's.
- **A release build SHRINKS, and what saves both plugins is upstream's rule,
  not ours.** `register_android_plugin(package, "KeystorePlugin")` is the only
  path to either Kotlin class, so R8 sees no reference, and `proguard-wry.pro`
  keeps `org.edet.client.*` — one level, not the plugin packages. What keeps
  them is the `tauri` crate's own consumer rule, `-keep @TauriPlugin public
  class *` with its `@Command` methods, plus the same for `@InvokeArg`:
  MEASURED, by deleting a local keep file and rebuilding — both classes
  survive under their own names while `DeviceKeyStore$get$1` is renamed to
  `X`. So a keep of ours is dead weight, and the thing worth holding is the
  PROPERTY: `android-release` reads both classes back out of the dex, which
  goes red if an upstream version ever drops that rule. Only `release`
  minifies, so the debug APK every device walk uses says nothing about it.
- **Two Android plugins cannot both manage a bare `PluginHandle<R>`.** App
  state is keyed by TYPE, so the second `manage` is a no-op and every later
  call reaches the FIRST plugin's Kotlin class, with no error anywhere. Both
  are wrapped (`KeystoreHandle`, `BackgroundHandle`).
- **The device key is wrapped by a Keystore key and the blob lives under
  `noBackupFilesDir`**; a blob that exists and will not open is
  `UnwrapFailed` (`unwrap-failed:` prefix), never `null`, which would mint a
  new key over it and hide the recovery phrase.
- **A persistent flow adjacency is refused for what it buys**, not for
  determinism; the reservation map is read in the edge walk's order
  (`ReservedCursor`), or the build grows with reservation density (4.4 → 20 ms
  at 20,000).
- **A behaviour nobody turned on must draw nothing** (`Rng::chance(p <= 0.0)`),
  or a re-pin's diff is dominated by the draw. A pin whose diff cannot be read
  is a pin nobody reads.
- **The swarm is a population over the real transition function, judged by
  the real audit** (`crates/swarm`). Strategies emit INTENTS and the adapter
  builds envelopes; a control is the same seed with every treatment seat
  honest, aged the same ticks; `just swarm-pin` after a deliberate change is a
  diff to READ (a `refused` count at zero is a rule that stopped firing; a
  re-pin that moves only `state_root` is an encoding change and nothing
  economic); a tick is a block (`TickCache`); `q2` refuses a population with
  no treatment. `edet-state` dev-depends on `edet-swarm`, which depends on
  `edet-state`: nothing under `crates/state/src` may name `edet_swarm`.
  Conformance ports 7471 / 28921 / 29571.
- **The model generates and the kernel judges; no persona figure is a
  measurement** (`scripts/persona.py`, at least two model families of
  independent lineage, an identical narrative across draws is saturation; run
  by hand, never by a gate).
- **`stranger_key` yields SIXTEEN distinct keys** (`0xF0 | n & 0x0F`); a
  scene that seats more rows than that re-seats an old one. **An inline
  locale default with a placeholder is a TEMPLATE literal** (`${…}`), or the
  inline-English gate reads it as drift from `en.json`.
