//! The tick loop, the report, and the replay.
//!
//! **Every run is one seed.** Per-agent RNG derived from it, the tick order
//! fixed (agents in ascending index, intents in emission order, `BTreeMap`
//! everywhere), the whole run replayable bit-identically — so a violated
//! invariant reports the seed, the tick, the agent, the intent, the
//! transition, the signers and the invariant, and THAT report is the
//! deliverable. A finding nobody can replay is an anecdote.
//!
//! **One tick is one epoch.** The tick opens with `goto`, which runs the
//! sweep; every agent then acts once. A strategy that wants to act less often
//! draws a chance; one that wants to act more returns several intents. It
//! keeps "epochs of honesty" and "ticks" one number.

use std::collections::BTreeMap;

use edet_state::state::State;
use edet_state::types::*;

use crate::driver::{Audit, Driver};
use crate::intent::{compose, AgentLog, Intent, Outcome, MAX_INTENTS_PER_TICK};
use crate::metrics::{hex32, ArchetypeRow, Distribution, Metrics, Summary, FORMAT_VERSION};
use crate::population::{Population, World};
use crate::rng::{mix, Rng};
use crate::strategy::{AgentView, Strategy, TickCache};

/// One run's configuration.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Run {
    pub seed: u64,
    pub population: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members: Option<usize>,
    pub ticks: u64,
    pub audit: Audit,
    /// How often the capacity distribution is taken. It costs one cut per
    /// member, so it is not a per-tick reading.
    pub metrics_every: u64,
    /// Whether to ask, of every row the ledger left uninsured, whether the
    /// PRISTINE cut would have carried it.
    ///
    /// Off by default and on for `q2`, because it is one more cut per
    /// uninsured row — which at farm sizes is the whole run, spent on a
    /// distributional reading the corpus does not pin.
    #[serde(default)]
    pub measure_ceiling: bool,
}

impl Run {
    /// The mode the corpus runs in, and the only mode a test may use.
    pub fn corpus(seed: u64, population: &str, ticks: u64) -> Self {
        Run {
            seed,
            population: population.to_string(),
            members: None,
            ticks,
            audit: Audit::EveryTransition,
            metrics_every: 30,
            measure_ceiling: false,
        }
    }

    pub fn replay_command(&self, until: u64) -> String {
        format!(
            "edet-swarm replay --seed {} --population {} --ticks {} --until {until}",
            self.seed, self.population, self.ticks
        )
    }
}

/// A violated invariant, with everything needed to reproduce it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Violation {
    pub seed: u64,
    pub tick: u64,
    pub agent: usize,
    pub archetype: String,
    pub intent: String,
    pub tx: String,
    pub signers: Vec<MemberId>,
    pub invariant: String,
    pub replay: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "seed {} broke an invariant at tick {}: agent {} ({}) emitted {}, signed by {:?}, and the audit answered: {}. Reproduce with `{}`. The transaction was {}.",
            self.seed,
            self.tick,
            self.agent,
            self.archetype,
            self.intent,
            self.signers,
            self.invariant,
            self.replay,
            self.tx
        )
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Report {
    pub summary: Summary,
    pub ticks: Vec<Metrics>,
    pub violation: Option<Violation>,
    /// **A rolling digest over every transition the run submitted**, in order:
    /// the consensus encoding of each `Tx`, then the outcome it came to.
    ///
    /// Stronger than the state root and answering a different question. The
    /// root says two runs ended in the same state; this says they took the
    /// same path there, refusals included — which is what "replayable
    /// bit-identically" has to mean for a violation report to be worth
    /// anything. It is deliberately NOT part of the pinned `Summary`: an id
    /// counter is in every envelope, so the digest is a claim about one
    /// process rather than about behaviour.
    pub transition_digest: String,
}

/// What one agent did over the whole run.
struct Agent {
    strategy: Box<dyn Strategy>,
    rng: Rng,
    log: Vec<AgentLog>,
    over_budget: u64,
}

impl Agent {
    /// Split borrow: acting and consenting both need the strategy and its own
    /// stream at once.
    fn parts(&mut self) -> (&mut dyn Strategy, &mut Rng) {
        (self.strategy.as_mut(), &mut self.rng)
    }
}

/// Run one seed to the end, or to the first violated invariant.
pub fn run(cfg: &Run) -> Result<Report, String> {
    let mut pop =
        Population::named(&cfg.population).ok_or_else(|| format!("no population named {}", cfg.population))?;
    if let Some(n) = cfg.members {
        pop.resize(n);
    }
    Ok(run_population(cfg, &pop, cfg.ticks))
}

/// The same, against a population built in code, stopped at `until` — what a
/// replay and a control need.
pub fn run_population(cfg: &Run, pop: &Population, until: u64) -> Report {
    let (d, mut world, strategies) = pop.genesis(cfg.audit);
    let mut d = d.report_violations();
    let genesis_members = d.st.members.len() as u64;
    let genesis_epoch = d.st.epoch;

    let mut agents: Vec<Agent> = strategies
        .into_iter()
        .enumerate()
        .map(|(i, s)| Agent { strategy: s, rng: Rng::seeded(mix(cfg.seed, i as u64)), log: Vec::new(), over_budget: 0 })
        .collect();

    let mut cache = TickCache::default();
    let mut ticks: Vec<Metrics> = Vec::new();
    let mut violation: Option<Violation> = None;
    let mut totals = Totals::default();
    let mut trace = Trace::default();
    let last_tick = until.min(cfg.ticks);

    'outer: for tick in 1..=last_tick {
        cache.clear();
        d.goto(genesis_epoch + tick);
        if let Some(v) = d.violation() {
            violation = Some(sweep_violation(cfg, tick, v));
            break;
        }
        let mut m = Metrics { tick, epoch: d.st.epoch, ..Default::default() };
        let mut attempted = 0u64;
        let mut applied = 0u64;

        for i in 0..agents.len() {
            let intents = {
                let view = AgentView { st: &d.st, world: &world, cache: &cache, me: i as MemberId, agent: i, tick };
                let (s, rng) = agents[i].parts();
                s.act(&view, rng)
            };
            agents[i].over_budget += intents.len().saturating_sub(MAX_INTENTS_PER_TICK) as u64;
            for intent in intents.into_iter().take(MAX_INTENTS_PER_TICK) {
                let books = crate::intent::books_a_row(&intent);
                if books {
                    attempted += 1;
                }
                let outcome = emit(&mut d, &mut world, &mut agents, &cache, i, tick, &intent, &mut m, cfg, &mut trace);
                match &outcome {
                    Outcome::Applied => {
                        if books {
                            applied += 1;
                        }
                    }
                    Outcome::Refused(code) => *m.refused.entry(code.to_string()).or_default() += 1,
                    Outcome::Declined(_) => m.declined += 1,
                    Outcome::Unbuilt(_) => m.unbuilt += 1,
                }
                if let Some(v) = d.violation() {
                    violation = Some(transition_violation(cfg, tick, i, &agents[i], &d, &intent, v));
                    break 'outer;
                }
            }
        }

        if cfg.audit == Audit::EveryTick {
            d.audit_now("at the end of a tick");
            if let Some(v) = d.violation() {
                violation = Some(sweep_violation(cfg, tick, v));
                break;
            }
        }

        for (i, a) in agents.iter_mut().enumerate() {
            let view = AgentView { st: &d.st, world: &world, cache: &cache, me: i as MemberId, agent: i, tick };
            let log = std::mem::take(&mut a.log);
            a.strategy.observe(&view, &log);
            a.log = log;
        }

        close_tick(&d.st, &mut m, attempted, applied);
        if cfg.metrics_every > 0 && (tick % cfg.metrics_every == 0 || tick == last_tick) {
            m.capacity = Some(Distribution::of(&d.st));
        }
        totals.add(&m);
        ticks.push(m);
    }

    let summary = summarise(&d.st, &ticks, &totals, &agents, genesis_members);
    Report { summary, ticks, violation, transition_digest: trace.finish() }
}

/// The rolling digest of everything submitted and everything the ledger
/// answered.
#[derive(Default)]
struct Trace(Vec<u8>);

impl Trace {
    fn push(&mut self, tx: &edet_state::tx::Tx) {
        self.fold(&edet_state::codec::encode(tx).unwrap_or_default());
    }

    fn answer(&mut self, outcome: &Outcome) {
        let tag: Vec<u8> = match outcome {
            Outcome::Applied => b"ok".to_vec(),
            Outcome::Refused(c) => format!("refused:{c}").into_bytes(),
            Outcome::Declined(m) => format!("declined:{m}").into_bytes(),
            Outcome::Unbuilt(w) => format!("unbuilt:{w}").into_bytes(),
        };
        self.fold(&tag);
    }

    /// Folded rather than accumulated, so the digest costs one hash per step
    /// and no memory at all — a run of a hundred thousand transitions must not
    /// have to keep them.
    fn fold(&mut self, bytes: &[u8]) {
        use edet_state::root::value_digest;
        let mut buf = std::mem::take(&mut self.0);
        buf.extend_from_slice(bytes);
        self.0 = value_digest(&buf).to_vec();
    }

    fn finish(&self) -> String {
        let mut out = [0u8; 32];
        for (i, b) in self.0.iter().take(32).enumerate() {
            out[i] = *b;
        }
        hex32(&out)
    }
}

fn sweep_violation(cfg: &Run, tick: u64, v: &str) -> Violation {
    Violation {
        seed: cfg.seed,
        tick,
        agent: usize::MAX,
        archetype: "the epoch sweep".into(),
        intent: "-".into(),
        tx: "-".into(),
        signers: Vec::new(),
        invariant: v.to_string(),
        replay: cfg.replay_command(tick),
    }
}

fn transition_violation(
    cfg: &Run,
    tick: u64,
    agent: usize,
    a: &Agent,
    d: &Driver,
    intent: &Intent,
    v: &str,
) -> Violation {
    let last = a.log.last().and_then(|l| l.envelope.as_ref());
    Violation {
        seed: cfg.seed,
        tick,
        agent,
        archetype: a.strategy.name().to_string(),
        intent: intent.kind().to_string(),
        tx: last.map(|e| format!("{:?}", e.tx)).unwrap_or_else(|| "-".into()),
        signers: last
            .map(|e| e.signers.iter().filter_map(|k| d.st.member_of_key(k)).collect())
            .unwrap_or_default(),
        invariant: v.to_string(),
        replay: cfg.replay_command(tick),
    }
}

/// Build one envelope, ask everybody it names, and apply it.
#[allow(clippy::too_many_arguments)]
fn emit(
    d: &mut Driver,
    world: &mut World,
    agents: &mut [Agent],
    cache: &TickCache,
    agent: usize,
    tick: u64,
    intent: &Intent,
    m: &mut Metrics,
    cfg: &Run,
    trace: &mut Trace,
) -> Outcome {
    // A replay is the actor's own earlier envelope, resubmitted verbatim:
    // same id, same window, same signers. Nothing is composed.
    if let Intent::Replayed { emission } = intent {
        let env = agents[agent].log.get(*emission).and_then(|l| l.envelope.clone());
        let Some(env) = env else {
            let outcome = Outcome::Unbuilt("no such emission");
            trace.answer(&outcome);
            return log(agents, agent, tick, intent, None, outcome, None);
        };
        trace.push(&env.tx);
        let out = d.apply_raw(env.tx.clone(), env.id, env.not_after, &env.signers);
        let outcome = match out {
            Ok(()) => Outcome::Applied,
            Err(e) => Outcome::Refused(e.0),
        };
        trace.answer(&outcome);
        return log(agents, agent, tick, intent, Some(env), outcome, None);
    }

    let actor = agent as MemberId;
    let composed = compose(&d.st, world, actor, intent);
    let mut signers = composed.signers;
    for asked in &composed.asks {
        let owner = world.agent_of(asked.ask.member);
        let granted = owner != usize::MAX && {
            let view = AgentView { st: &d.st, world, cache, me: asked.ask.member, agent: owner, tick };
            let (s, rng) = agents[owner].parts();
            s.consents(&view, &asked.ask, rng)
        };
        if granted {
            signers.push(asked.key);
        } else if asked.required {
            // A DECLINE, which is the pending-signature pool and not `apply`.
            let outcome = Outcome::Declined(asked.ask.member);
            trace.answer(&outcome);
            return log(agents, agent, tick, intent, None, outcome, None);
        }
    }
    signers.sort_by_key(|k| d.st.member_of_key(k).unwrap_or(MemberId::MAX));
    signers.dedup();

    let contract_before = d.st.next_contract;
    let member_before = d.st.next_member;
    trace.push(&composed.tx);
    let out = d.apply(composed.tx, &signers);
    let envelope = d.last_envelope().cloned();
    let outcome = match out {
        Ok(()) => Outcome::Applied,
        Err(e) => Outcome::Refused(e.0),
    };
    trace.answer(&outcome);
    let mut contract = None;
    if outcome == Outcome::Applied {
        if d.st.next_contract > contract_before {
            contract = Some(contract_before);
            measure_row(d, m, contract_before, cfg);
        }
        // Ids are dense and never reused, so the rows this transition seated
        // are exactly the ones past the counter it started at.
        for id in member_before..d.st.next_member {
            world.claim(id, agent);
            m.seated += 1;
        }
    }
    log(agents, agent, tick, intent, envelope, outcome, contract)
}

fn log(
    agents: &mut [Agent],
    agent: usize,
    tick: u64,
    intent: &Intent,
    envelope: Option<crate::driver::Envelope>,
    outcome: Outcome,
    contract: Option<ContractId>,
) -> Outcome {
    agents[agent]
        .log
        .push(AgentLog { tick, intent: intent.clone(), envelope, outcome: outcome.clone(), contract });
    outcome
}

fn measure_row(d: &Driver, m: &mut Metrics, id: ContractId, cfg: &Run) {
    let Some(c) = d.st.contracts.get(&id) else { return };
    m.rows_accepted += 1;
    m.accepted_minor += c.original;
    if c.insured {
        m.insured_minor += c.original;
        return;
    }
    // **A binding ceiling withholds insurance, not trade.** The pristine cut
    // would have carried this row and the live reservations did not — the
    // reading no per-member figure shows, and one more cut per uninsured row,
    // which is why it is asked for only when somebody wants the answer.
    if cfg.measure_ceiling && d.st.gross_capacity_of_set_minor(&[c.debtor]) >= c.original {
        m.ceiling_uninsured += 1;
    }
}

#[derive(Default)]
struct Totals {
    rows_accepted: u64,
    accepted_minor: u64,
    insured_minor: u64,
    refused: BTreeMap<String, u64>,
    declined: u64,
    unbuilt: u64,
    deadlocked: u64,
    ceiling_uninsured: u64,
}

impl Totals {
    fn add(&mut self, m: &Metrics) {
        self.rows_accepted += m.rows_accepted;
        self.accepted_minor += m.accepted_minor;
        self.insured_minor += m.insured_minor;
        for (k, v) in &m.refused {
            *self.refused.entry(k.clone()).or_default() += v;
        }
        self.declined += m.declined;
        self.unbuilt += m.unbuilt;
        self.ceiling_uninsured += m.ceiling_uninsured;
        if m.deadlocked {
            self.deadlocked += 1;
        }
    }
}

fn close_tick(st: &State, m: &mut Metrics, attempted: u64, applied: u64) {
    m.members = st.members.len() as u64;
    m.drawn_minor = st.members.values().map(|x| x.debt_out).sum();
    m.committed_minor = edet_kernel::flow::committed_total(&st.committed);
    m.expired_rows = st.contracts.values().filter(|c| c.status == ContractStatus::Expired).count() as u64;
    m.open_default_minor = st.members.values().map(|x| x.rep.open_default).sum();
    m.forfeited_minor = st.forfeit_reserve.values().sum();
    m.exits = st.members.values().filter(|x| x.status == MemberStatus::Exited).count() as u64;
    m.enacted = st.proposals.values().filter(|p| p.enacted).count() as u64;
    m.denied_members = st.members.values().filter(|x| x.bond_denied_this_epoch).count() as u64;
    // **A tick in which rows were attempted and none applied.** The
    // distributional reading a per-member figure cannot show.
    m.deadlocked = attempted > 0 && applied == 0;
}

fn summarise(st: &State, ticks: &[Metrics], totals: &Totals, agents: &[Agent], genesis_members: u64) -> Summary {
    let mut archetypes: BTreeMap<String, ArchetypeRow> = BTreeMap::new();
    for a in agents {
        let row = archetypes.entry(a.strategy.name().to_string()).or_default();
        let first = row.agents == 0;
        row.agents += 1;
        row.emitted += a.log.len() as u64;
        row.over_budget += a.over_budget;
        for l in &a.log {
            match &l.outcome {
                Outcome::Applied => row.applied += 1,
                Outcome::Refused(c) => *row.refused.entry(c.to_string()).or_default() += 1,
                Outcome::Declined(_) => row.declined += 1,
                Outcome::Unbuilt(_) => row.unbuilt += 1,
            }
        }
        // **Every agent of an archetype has to reach it**, or the row says the
        // archetype did not — and the detail carried is the one that failed,
        // because that is the reading somebody has to act on.
        let p = a.strategy.probe();
        if first {
            row.probe_what = p.what.to_string();
            row.probe_reached = p.reached;
            row.probe_detail = p.detail;
        } else {
            if !p.reached && row.probe_reached {
                row.probe_detail = p.detail;
            }
            row.probe_reached &= p.reached;
        }
    }
    let last = ticks.last();
    Summary {
        format_version: FORMAT_VERSION,
        ticks: ticks.len() as u64,
        members_final: st.members.len() as u64,
        seated: (st.members.len() as u64).saturating_sub(genesis_members),
        rows_accepted: totals.rows_accepted,
        accepted_minor: totals.accepted_minor,
        insured_minor: totals.insured_minor,
        committed_final: edet_kernel::flow::committed_total(&st.committed),
        seat_committed_final: edet_kernel::flow::committed_total(&st.seat_committed),
        external_seed: st.underwriters.values().sum(),
        refused: totals.refused.clone(),
        declined: totals.declined,
        unbuilt: totals.unbuilt,
        expired_rows: last.map(|m| m.expired_rows).unwrap_or(0),
        open_default_final: last.map(|m| m.open_default_minor).unwrap_or(0),
        forfeited_final: last.map(|m| m.forfeited_minor).unwrap_or(0),
        exits: last.map(|m| m.exits).unwrap_or(0),
        enacted: last.map(|m| m.enacted).unwrap_or(0),
        denied_members: st.members.values().filter(|x| x.bond_saturated_epochs > 0).count() as u64,
        deadlocked_ticks: totals.deadlocked,
        ceiling_uninsured: totals.ceiling_uninsured,
        capacity: Distribution::of(st),
        archetypes,
        state_root: edet_state::root::state_root(st)
            .map(|r| hex32(&r))
            .unwrap_or_else(|e| format!("unrooted: {e:?}")),
    }
}
