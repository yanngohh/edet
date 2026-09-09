//! **One test per archetype, each asserting the probe that archetype exists
//! to reach**, and each naming in its doc comment the mutation to the state
//! machine that turns it red. Every one of those mutations has been applied by
//! hand once: a green suite that only asserts a gate PASSES says nothing about
//! what it DETECTS.
//!
//! An archetype that never manages to emit the transition it exists to test is
//! scenery, so every test also holds its row to `emitted > 0` and
//! `applied > 0`.

use edet_swarm::metrics::{ArchetypeRow, Summary};
use edet_swarm::run::{run, Run};

/// Run a preset and hand back what one archetype did in it.
fn row(population: &str, seed: u64, ticks: u64, archetype: &str) -> (Summary, ArchetypeRow) {
    let cfg = Run::corpus(seed, population, ticks);
    let report = run(&cfg).expect("a named population");
    assert!(report.violation.is_none(), "{population}: {:?}", report.violation);
    let row = report
        .summary
        .archetypes
        .get(archetype)
        .unwrap_or_else(|| panic!("{population} seats no {archetype}"))
        .clone();
    assert!(row.emitted > 0, "{archetype} emitted nothing — it is scenery in {population}");
    assert!(row.applied > 0, "{archetype} applied nothing — it is scenery in {population}");
    (report.summary, row)
}

fn reached(r: &ArchetypeRow, archetype: &str) {
    assert!(r.probe_reached, "{archetype} did not reach its probe ({}): {}", r.probe_what, r.probe_detail);
}

/// **An all-honest population works**: nothing falls into default, and the
/// community insures a real share of what it trades.
///
/// The per-agent half is in the archetype's own probe; this is the half that
/// is a statement about the population, which no single agent can make.
///
/// Mutation that bites: drop the maturity floor from `accept` —
/// `refused["ET-CTR-005"]` goes to zero, because the honest trader draws a
/// term one epoch below it on purpose at `junk_rate`. Without that deliberate
/// junk the mutation changes no summary anywhere in the corpus.
#[test]
fn an_honest_population_defaults_on_nothing_and_insures_what_it_trades() {
    let (summary, r) = row("honest", 1, 120, "honest");
    reached(&r, "honest");
    assert_eq!(summary.expired_rows, 0, "an honest population must reach the end with nothing in default");
    assert!(summary.insured_minor > 0, "and the community must actually insure some of it");
    assert!(
        summary.refused.get("ET-CTR-005").copied().unwrap_or(0) > 0,
        "the maturity floor must FIRE somewhere in an ordinary run, or nothing detects its removal"
    );
    assert_eq!(summary.capacity.non_conferrer_share.0, 0, "and standing must circulate rather than pool");
}

/// **A default expires through the sweep, keeps its hold, and closes the
/// defaulter's own budget.**
///
/// Mutation that bites: call `flow::release` on the row's `Held` inside
/// `mark_expired` — the hold falls at the expiry and the probe says so; or
/// drop the expiry pass from `sweep_cranks` — no row ever expires and the
/// probe never reaches.
#[test]
fn a_deadbeat_expires_through_the_sweep_and_the_hold_is_kept() {
    let (summary, r) = row("deadbeats", 2, 120, "deadbeat");
    reached(&r, "deadbeat");
    assert!(summary.expired_rows > 0);
    assert!(summary.open_default_final > 0);
}

/// **A ring cannot turn manufactured standing into a supply**, and its insured
/// credit AS A SET stays inside the cut that reaches it.
///
/// Mutation that bites: let `declare_supply` admit a raise up to the
/// declarer's own capacity — the hollow-insurance construction. Every raise
/// then applies instead of coming back `ET-UWR-001`, and the ring's insured
/// credit passes its own cut within a few ticks.
#[test]
fn a_wash_ring_is_refused_every_raise_and_stays_inside_its_own_cut() {
    let (summary, r) = row("wash-ring", 3, 120, "wash-ring");
    reached(&r, "wash-ring");
    assert!(
        summary.refused.get("ET-UWR-001").copied().unwrap_or(0) > 0,
        "the raise refusal must FIRE, or nothing detects its removal"
    );
}

/// **A row is a stock, and its price is a reservation on the graph.** The rows
/// this farm seats stay inside the cut behind its own set, tick by tick, and
/// the ceiling it stops at IS that cut.
///
/// A seat holds one bond unit of flow from the community's seed to the
/// sponsor, on the shared stake graph, and nothing releases it — so
/// `unit x seats(S)` can never exceed the all-time peak of the arcs reaching
/// `S` from outside. Measured: **25 rows behind one edge of 500.00, 250 behind
/// 5,000.00**, and the honest preset's seed of 7,500.00 seats **375** rows ever
/// without a ceremony.
///
/// **Both halves of the probe are load-bearing.** Rows inside the bound is the
/// safety claim; `ET-BND-006` firing is what makes the probe a detector — a
/// farm that never met its ceiling would pass with the mechanism gone.
///
/// Mutations that bite, each run by hand once:
///
///   - point `State::reserve_seat` at a throwaway map: invariant 7 refuses the
///     first seat, and with its conservation clauses dropped too the gate never
///     refuses at all and the same edge carries another 25 rows every epoch;
///   - make `fresh_keys` a flag rather than a count: the write still takes one
///     seat per fresh party, so what breaks is the gate — `admits` says yes to
///     a trade `apply` then refuses;
///   - floor `flow::decay` at `seat_reserved`: the arcs under the seats stop
///     fading and the sponsor keeps the whole edge of capacity for ever.
#[test]
fn a_sybil_farm_stays_inside_the_cut_behind_its_set() {
    let (summary, r) = row("sybil-farm", 4, 5, "sybil-farm");
    reached(&r, "sybil-farm");
    assert!(
        summary.refused.get("ET-BND-006").copied().unwrap_or(0) > 0,
        "the seat ceiling must FIRE, or nothing detects its removal"
    );
}

/// **Ten times the backing, and the count follows it.** A rule that merely
/// re-scaled the price would look identical at one size, which is why the
/// claim is measured at two.
#[test]
fn a_sybil_farm_at_ten_times_the_backing_seats_ten_times_the_rows() {
    let (small, _) = row("sybil-farm", 4, 5, "sybil-farm");
    let (large, r) = row("sybil-farm-x10", 14, 8, "sybil-farm");
    reached(&r, "sybil-farm");
    assert!(
        large.seat_committed_final >= 9 * small.seat_committed_final,
        "ten times the backing must carry about ten times the rows: {} against {}",
        large.seat_committed_final,
        small.seat_committed_final
    );
}

/// **Time buys nothing.** Three hundred ticks of the same farm, renewing the
/// same edge, hold exactly the rows five ticks did: the bound is over all-time
/// PEAKS, so decay frees no seat and renewal to an old peak opens none.
#[test]
fn a_sybil_farm_holds_the_same_rows_after_three_hundred_ticks() {
    let (short, _) = row("sybil-farm", 4, 5, "sybil-farm");
    let (long, r) = row("sybil-farm-long", 15, 300, "sybil-farm");
    reached(&r, "sybil-farm");
    assert_eq!(long.seat_committed_final, short.seat_committed_final, "sixty times the run must not seat one more row");
}

/// **The community's own ceiling never stops an ordinary newcomer stream.**
/// A seat that cost honest onboarding anything would be a tax on the one act
/// that grows the graph, so what refuses one has to be the SPONSOR's own
/// backing and never the seed.
///
/// Measured on this preset, greeting at a tenth of its ticks: 113 rows seated
/// over 120 ticks against a seed that carries 375, and the community's ceiling
/// untouched throughout.
///
/// **What a sponsor does meet, eventually, is their own.** Standing fades
/// unless trade renews it and a seat is priced on present standing, so a member
/// greeting a twelfth of the time for 120 ticks runs out of reach as their
/// genesis edge decays — zero refusals at 30 and 60 ticks, two at 120. That is
/// the same rule the bound is stated over, read one member at a time, and the
/// corpus pins the count.
#[test]
fn the_seed_never_refuses_an_honest_communitys_newcomers() {
    let (summary, r) = row("honest-open", 16, 120, "honest");
    reached(&r, "honest");
    assert!(summary.seated > 0, "the population must actually bring newcomers in");
    assert!(
        summary.seat_committed_final < summary.external_seed,
        "the community's ceiling must never be what binds: {} of {}",
        summary.seat_committed_final,
        summary.external_seed
    );
}

/// **And the ceiling binds exactly where the seed runs out.** A bound that
/// never binds is not one, and one that binds anywhere else is not this one.
#[test]
fn a_community_greeting_every_tick_seats_its_seed_over_the_unit_and_then_nobody() {
    let (summary, r) = row("honest-full", 17, 120, "honest");
    reached(&r, "honest");
    assert_eq!(
        summary.seat_committed_final, summary.external_seed,
        "every unit of the seed must end up spent on a row"
    );
    assert!(
        summary.refused.get("ET-BND-006").copied().unwrap_or(0) > 0,
        "and the next seat after that must be refused"
    );
}

/// The same archetype inside a mixed population, washed BELOW the
/// establishment floor — where the seated rows qualify for no allowance at all
/// and the seat is still what bounds the count.
///
/// The two thresholds sit within a fifth of each other at the genesis
/// parameters, which is what makes it easy to attribute one channel's work to
/// the other's constant. This entry holds them apart: the establishment floor
/// withholds the ALLOWANCE, the seat withholds the ROW.
#[test]
fn a_sybil_farm_below_the_establishment_floor_stays_inside_its_cut() {
    let (_, r) = row("everything", 12, 60, "sybil-farm");
    reached(&r, "sybil-farm");
}

/// **A failed transaction still pays, an envelope that authorises nothing does
/// not, and a crank that found nothing spends no id.**
///
/// Mutation that bites: remove `refund(state, charged)` from
/// `apply_with_cache` — the stripped envelope moves its signer's encumbrance
/// and the probe reports it billed somebody; remove `forget_applied` on the
/// `ET-MEM-003` branch — the id is spent, so `strip_unbilled` never sets and
/// the same transaction properly signed comes back `ET-TX-001`; drop the
/// permissionless branch — a crank on a contract that does not exist writes an
/// id into state the root hashes.
#[test]
fn a_griefer_is_denied_and_its_stripped_envelope_is_unbilled() {
    let (summary, r) = row("griefer", 5, 60, "griefer");
    reached(&r, "griefer");
    assert!(summary.refused.get("ET-BND-001").copied().unwrap_or(0) > 0, "the gate must actually deny");
    assert!(summary.refused.get("ET-MEM-003").copied().unwrap_or(0) > 0, "and the strip must actually be refused");
    assert!(summary.forfeited_final > 0, "three saturated epochs must reach the forfeiture crank");
}

/// **A change to who ORDERS the ledger takes two thirds, everything else a
/// half** — and the class is read off the TARGET's voting power, not off the
/// proposal's kind.
///
/// Mutation that bites: key `adoption_threshold` on the kind alone — the
/// suspension of a sitting validator then enacts at the half, and the
/// coalition at 0.55 of the seed carries it.
#[test]
fn a_coalition_enacts_exactly_at_the_threshold_its_target_puts_it_in() {
    for (preset, seed) in [("coalition-third", 6), ("coalition-half", 7), ("coalition-two-thirds", 8)] {
        let (_, r) = row(preset, seed, 30, "coalition");
        reached(&r, preset);
    }
}

/// **`Exit` succeeds at most once, a Suspended member may still take it, and
/// the stakes on the leaver stay in the graph.**
///
/// Mutation that bites: admit a second `Exit` on an `Exited` row — the probe's
/// "every later write refused" fails at once, since the archetype emits one
/// every tick after it leaves.
#[test]
fn an_exiter_leaves_once_under_sanction_and_leaves_its_stakes_behind() {
    let (summary, r) = row("exit-under-suspension", 9, 30, "exiter");
    reached(&r, "exiter");
    assert_eq!(summary.exits, 1);
    assert!(summary.refused.get("ET-MEM-002").copied().unwrap_or(0) > 0);
}

/// **What a sleeper extracts is bounded by the cut in front of it at the
/// moment it defects, whatever `E` was.**
///
/// Across four values of `E` in one family, because the claim is that the
/// bound is in AMOUNT and not in time: a single `E` cannot tell a bound from
/// a coincidence.
///
/// Mutation that bites: release committed flow when a row expires, or let
/// `flow::decay` touch `committed` — the sleeper's capacity comes back a term
/// later, it re-borrows, and what it extracts grows with `E`.
#[test]
fn a_sleeper_is_bounded_in_amount_and_not_in_time() {
    for (i, e) in [30u64, 90, 180].iter().enumerate() {
        let (_, r) = row(&format!("sleeper-debtor-{e}"), 20 + i as u64, e + 60, "sleeper");
        reached(&r, "sleeper-debtor");
        let (_, r) = row(&format!("sleeper-underwriter-{e}"), 30 + i as u64, e + 60, "sleeper");
        reached(&r, "sleeper-underwriter");
    }
}

/// **An award is the median over the whole WINDOW, minted by the sweep at its
/// close** — and what it is a median OF is the attestations the window
/// received.
///
/// With every panel member attesting and the colluders a minority, that median
/// is zero and nothing mints. With one honest arbiter silent it is not: an
/// arbiter who does not attest hands the median to whoever did, which is why
/// the quorum both parties consent to is as much of the terms as the panel.
///
/// Mutation that bites: mint at quorum inside `arb_attest` — the colluders'
/// median mints on their second signature, before the honest majority has
/// attested at all, and `awarded_early` catches it.
#[test]
fn an_award_waits_for_the_window_and_then_mints_the_median_it_received() {
    let (_, r) = row("late-defaulter", 10, 150, "late-defaulter");
    reached(&r, "late-defaulter");
}

/// **Standing that confers nothing is the failure mode that looks healthy in
/// every aggregate**, so the summary reports it as a share.
#[test]
fn hoarders_hold_capacity_and_confer_none_of_it() {
    let (summary, r) = row("hoarders", 11, 120, "hoarder");
    reached(&r, "hoarder");
    assert!(summary.capacity.non_conferrer_share.0 > 0, "the hoarders' own capacity is exactly this share");
}

/// **Every archetype reaches its probe in one run**, which is the only run in
/// which one archetype's behaviour is another's environment.
#[test]
fn the_everything_preset_reaches_every_probe_in_one_run() {
    let cfg = Run::corpus(12, "everything", 200);
    let report = run(&cfg).expect("a named population");
    assert!(report.violation.is_none(), "{:?}", report.violation);
    for (name, r) in &report.summary.archetypes {
        assert!(r.emitted > 0 && r.applied > 0, "{name} is scenery here");
        assert_eq!(r.over_budget, 0, "{name} went past the per-tick intent bound: the run reports its own harness");
        assert!(r.probe_reached, "{name} did not reach its probe ({}): {}", r.probe_what, r.probe_detail);
    }
    assert_eq!(report.summary.archetypes.len(), 10, "every archetype must be seated here");
}
