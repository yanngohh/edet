//! **A finding nobody can replay is an anecdote.** These are the probes for
//! the property every other test in this crate rests on.

use edet_swarm::driver::Audit;
use edet_swarm::population::Population;
use edet_swarm::run::{run, run_population, Run};

/// One seed, twice in one process, identical down to the order of the
/// refusals.
///
/// The summary alone is not enough and the digest is what says so: two runs
/// that took different paths to the same state agree on `state_root` and
/// disagree here, which is exactly the case a violation report has to survive.
///
/// Mutation that bites: give every agent one shared stream instead of
/// `mix(seed, index)` — the two runs still agree, and then adding an observer
/// to the loop changes the run and the reported seed stops reproducing.
#[test]
fn one_seed_replays_bit_identically() {
    let cfg = Run::corpus(7, "everything", 25);
    let a = run(&cfg).expect("a named population");
    let b = run(&cfg).expect("a named population");
    assert_eq!(a.summary, b.summary, "the same seed must produce the same summary");
    assert_eq!(
        a.transition_digest, b.transition_digest,
        "the same seed must submit the same transitions in the same order and get the same answers"
    );
    assert!(a.summary.accepted_minor > 0, "a run that booked nothing measures nothing");
}

/// Stopping early is a PREFIX, which is what `replay --until TICK` promises.
#[test]
fn a_replay_stopped_early_is_a_prefix_of_the_whole_run() {
    let cfg = Run::corpus(7, "deadbeats", 40);
    let pop = Population::named("deadbeats").expect("a named population");
    let whole = run_population(&cfg, &pop, 40);
    let part = run_population(&cfg, &pop, 20);
    assert_eq!(part.ticks.len(), 20);
    // All but the last, which is not a prefix of anything: the capacity
    // distribution is taken at the END of a run whatever `metrics_every` says,
    // so the tick a run stops on carries a reading the same tick in a longer
    // run does not.
    assert_eq!(&whole.ticks[..19], &part.ticks[..19], "the first ticks must not depend on how many follow");
    assert!(part.ticks[19].capacity.is_some(), "the last tick of a run always carries the distribution");
    assert!(whole.ticks[19].capacity.is_none(), "and a tick in the middle of one does not");
}

/// **A control is the same seed with every treatment seat honest, aged the
/// same ticks.** It has to differ from the treatment, or every Q2 ratio is
/// one.
#[test]
fn a_control_is_the_same_population_with_the_treatment_seats_honest() {
    let cfg = Run::corpus(11, "deadbeats", 60);
    let pop = Population::named("deadbeats").expect("a named population");
    let treatment = run_population(&cfg, &pop, 60);
    let control = run_population(&cfg, &pop.control(), 60);
    assert_eq!(control.summary.members_final, treatment.summary.members_final, "the same seats");
    assert_eq!(control.summary.ticks, treatment.summary.ticks, "aged identically");
    assert!(treatment.summary.expired_rows > 0, "the treatment must actually default");
    assert_eq!(control.summary.expired_rows, 0, "and the control must not");
    assert!(
        !control.summary.archetypes.contains_key("deadbeat"),
        "a control has no treatment seats left to be a control OF"
    );
}

/// The report a search hands back names the seed, the tick, the agent and the
/// command that reproduces it — because that report is the deliverable.
#[test]
fn a_violation_report_carries_its_own_replay_command() {
    let cfg = Run::corpus(3, "wash-ring", 5);
    let report = run(&cfg).expect("a named population");
    assert!(report.violation.is_none(), "this tree holds: {:?}", report.violation);
    // The shape, over a report built the way the loop builds one.
    let v = edet_swarm::run::Violation {
        seed: cfg.seed,
        tick: 4,
        agent: 6,
        archetype: "wash-ring".into(),
        intent: "Declare".into(),
        tx: "-".into(),
        signers: vec![6],
        invariant: "invariant 1".into(),
        replay: cfg.replay_command(4),
    };
    assert_eq!(v.replay, "edet-swarm replay --seed 3 --population wash-ring --ticks 5 --until 4");
    assert!(format!("{v}").contains("Reproduce with"));
}

/// **`Audit::Off` is for measurements**, and the corpus mode is not one.
#[test]
fn the_corpus_mode_audits_after_every_transition() {
    assert_eq!(Run::corpus(1, "honest", 10).audit, Audit::EveryTransition);
}

/// A tick's audit and a transition's audit reach the same verdict on the same
/// seed, which is what lets a search run in `EveryTick` and a replay name the
/// transition in `EveryTransition`.
#[test]
fn the_two_audit_modes_agree_on_the_same_seed() {
    let pop = Population::named("griefer").expect("a named population");
    let strict = Run::corpus(5, "griefer", 30);
    let mut fast = strict.clone();
    fast.audit = Audit::EveryTick;
    let a = run_population(&strict, &pop, 30);
    let b = run_population(&fast, &pop, 30);
    assert_eq!(a.violation.is_none(), b.violation.is_none());
    assert_eq!(a.summary.state_root, b.summary.state_root, "the audit mode may not change what the run DOES");
    assert_eq!(a.transition_digest, b.transition_digest);
}
