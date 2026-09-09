//! **The seed-sizing rehearsal**: a synthetic year of trade against a
//! candidate seed, reporting what a founding community actually has to choose.
//!
//! The seed is the one number a community cannot revise upward without a
//! second ceremony, and §Adoption's table gives the turnover rate but not the
//! rest of the answer. What a founder needs beside it is the shape of their own
//! trade: how much insured credit stands simultaneously at the peak, what share
//! of rows fall to the uninsured tier for want of capacity, how many rows the
//! write gate refuses outright — and, since a row is a stock priced by a seat
//! on the seed's reach, **how many people the seed carries at all**. A seat
//! holds one bond unit, so a community seats `Σ supply / unit` rows and then
//! nobody until a ceremony; the seat ceiling and the insurance ceiling are one
//! number in two units, and a founder sizing for credit is sizing for members
//! whether they know it or not.
//!
//! So this drives the real transition function through a warm-up year that
//! seats the founding members by trade and earns them standing, and a
//! measured year of trade with newcomers arriving through real seating trades,
//! members leaving, and — if a cadence is given — the community running its
//! seed-amendment ceremony on schedule. Over four candidate seeds it prints
//! the figures per seed. It is a REHEARSAL: the process is synthetic and its
//! parameters are the founder's guesses. What it removes is the class of error
//! where the guesses were never carried through the mechanism at all.
//!
//! `#[ignore]`d for the same reason `cost.rs` is: this is a measurement, not an
//! assertion, and its numbers are a function of the parameters. Run it with
//! `just size-seed`, which passes the environment through. The ordinary
//! probes at the bottom run in `just ci` on tiny parameters, because a
//! measurement harness that stops compiling is a measurement nobody makes.
//!
//! Parameters, all from the environment with the defaults named here:
//!
//! | variable | default | meaning |
//! |---|---|---|
//! | `EDET_SIZE_SEED` | `10000` | the candidate seed, shared equally |
//! | `EDET_SIZE_UNDERWRITERS` | `4` | how many share it |
//! | `EDET_SIZE_MEMBERS` | `200` | founding members, seated in the warm-up |
//! | `EDET_SIZE_TRADES_PER_EPOCH` | `20` | arrival rate of obligations |
//! | `EDET_SIZE_MEAN_AMOUNT` | `100` | mean obligation, uniform ±50% |
//! | `EDET_SIZE_TERMS` | `30:50,60:30,90:20` | term:percent mix |
//! | `EDET_SIZE_SETTLE_ON_TIME` | `95` | percent settled at maturity |
//! | `EDET_SIZE_CURE_AFTER` | `30` | epochs from default to cure |
//! | `EDET_SIZE_EPOCHS` | `365` | the measured year |
//! | `EDET_SIZE_ARRIVALS_PER_EPOCH` | `1` | newcomers seated per epoch, measured year |
//! | `EDET_SIZE_CHURN_PER_YEAR` | `0` | percent of live members who stop trading, per year |
//! | `EDET_SIZE_AMEND_EVERY` | `0` | epochs between seed ceremonies (0: none) |
//! | `EDET_SIZE_AMEND_FRACTION` | `1.0` | share of an epoch's rate headroom each ceremony admits |
//!
//! The shortest term the mix may name is the maturity floor: a term below
//! `params.min_maturity_epochs` is refused at acceptance and no `ParamKey`
//! reaches that field.
//!
//! **The rate headroom does not carry over.** `seed::remaining` is β times the
//! seed at the epoch's open, less what this epoch already admitted, and it
//! resets at the boundary; a ceremony every thirty epochs therefore grows the
//! seed by β per ceremony — 27% a year at β = 0.02 — and only a ceremony every
//! epoch compounds at β per epoch. The table prints that arithmetic beside the
//! measured figures.

use std::collections::{BTreeMap, BTreeSet};

use edet_state::errors::{Error, ET_BOND_EXHAUSTED, ET_BOND_SEAT_UNBACKED, ET_BOND_STATUS};
use edet_state::state::State;
use edet_state::tx::Tx;
use edet_state::types::{ContractId, ContractStatus, Key, MemberId, Party, ProposalKind};
use edet_swarm::driver::{Audit, Driver};
use edet_swarm::keys::{fresh_key, member_key as key};

// ------------------------------------------------------------------ inputs --

/// A parameter read from the environment, or its default.
fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// The term mix, as `(term_epochs, cumulative_percent)` pairs summing to 100.
///
/// Parsed rather than sampled from a distribution because a founder knows
/// their own terms and does not know a distribution's parameters.
fn env_terms(name: &str, default: &str) -> Vec<(u64, u64)> {
    let raw = std::env::var(name).unwrap_or_else(|_| default.to_string());
    let mut cumulative = 0;
    let mut out = Vec::new();
    for piece in raw.split(',') {
        let (term, percent) = piece.split_once(':').expect("a term mix is `term:percent,...`");
        cumulative += percent.trim().parse::<u64>().expect("a percent");
        out.push((term.trim().parse::<u64>().expect("a term in epochs"), cumulative));
    }
    assert_eq!(cumulative, 100, "the term mix must sum to 100 percent");
    out
}

/// The founder's guesses, in one place.
#[derive(Clone)]
struct Process {
    underwriters: u64,
    members: u64,
    trades_per_epoch: u64,
    mean_amount: f64,
    terms: Vec<(u64, u64)>,
    settle_on_time: u64,
    cure_after: u64,
    epochs: u64,
    arrivals_per_epoch: u64,
    churn_per_year: u64,
    amend_every: u64,
    amend_fraction: f64,
}

impl Process {
    fn from_env() -> Self {
        Process {
            underwriters: env_u64("EDET_SIZE_UNDERWRITERS", 4),
            members: env_u64("EDET_SIZE_MEMBERS", 200),
            trades_per_epoch: env_u64("EDET_SIZE_TRADES_PER_EPOCH", 20),
            mean_amount: env_u64("EDET_SIZE_MEAN_AMOUNT", 100) as f64,
            terms: env_terms("EDET_SIZE_TERMS", "30:50,60:30,90:20"),
            settle_on_time: env_u64("EDET_SIZE_SETTLE_ON_TIME", 95),
            cure_after: env_u64("EDET_SIZE_CURE_AFTER", 30),
            epochs: env_u64("EDET_SIZE_EPOCHS", 365),
            arrivals_per_epoch: env_u64("EDET_SIZE_ARRIVALS_PER_EPOCH", 1),
            churn_per_year: env_u64("EDET_SIZE_CHURN_PER_YEAR", 0),
            amend_every: env_u64("EDET_SIZE_AMEND_EVERY", 0),
            amend_fraction: env_f64("EDET_SIZE_AMEND_FRACTION", 1.0),
        }
    }

    /// The tiny process the ordinary probes run, so they cost a second rather
    /// than a minute and still exercise every path the report does.
    fn tiny() -> Self {
        Process {
            underwriters: 2,
            members: 12,
            trades_per_epoch: 3,
            mean_amount: 100.0,
            terms: vec![(30, 100)],
            settle_on_time: 100,
            cure_after: 30,
            epochs: 120,
            arrivals_per_epoch: 1,
            churn_per_year: 0,
            amend_every: 0,
            amend_fraction: 1.0,
        }
    }
}

// -------------------------------------------------------------- randomness --

/// xorshift64*, seeded by a constant.
///
/// Deterministic and in-tree: a rehearsal two people run must produce the same
/// figures, and a dependency on `rand` would put the process's shape outside
/// this file where nobody reading it would look.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A value in `0..n`.
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    /// A value in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> Option<T> {
        if items.is_empty() {
            None
        } else {
            Some(items[self.below(items.len() as u64) as usize])
        }
    }
}

// ------------------------------------------------------------------- driver --

/// What one run of the process measured.
#[derive(Default, Debug)]
struct Measured {
    /// The largest insured credit standing simultaneously, in denomination
    /// units — the quantity the seed is actually sized against.
    peak_insured: f64,
    /// Amount accepted, insured and in total.
    insured_amount: f64,
    accepted_amount: f64,
    /// Rows the write gate refused, by code.
    bond_refusals: u64,
    /// Rows refused for anything else — a capacity ceiling on the debtor's own
    /// debt, an amount below dust after the draw.
    other_refusals: u64,
    /// Rows seated by trade — the founding members in the warm-up and every
    /// arrival since.
    seats: u64,
    /// Seating trades the seat gate refused (`ET-BND-006`), and the first
    /// epoch it did, counted over the whole run.
    seat_refusals: u64,
    first_seat_refusal_epoch: Option<u64>,
    /// Members still trading at the end.
    live_rows: u64,
    /// The external seed at the end, and what the live seats hold on it.
    seed_final: f64,
    seat_committed_final: f64,
    /// Ceremonies enacted, and the seed they admitted.
    ceremonies: u64,
    seed_added: f64,
}

impl Measured {
    fn insured_share(&self) -> f64 {
        if self.accepted_amount <= 0.0 {
            0.0
        } else {
            self.insured_amount / self.accepted_amount
        }
    }
}

/// The community as it runs: the driver, and who is who.
///
/// **The driver is the tree's one driver.** A rehearsal with a driver of its
/// own is a second implementation of the thing every figure below is computed
/// from, and one that audits nothing can report a curve for a ledger no
/// invariant would accept.
struct Community {
    d: Driver,
    underwriters: Vec<MemberId>,
    /// Every row a trade seated, in seating order.
    members: Vec<MemberId>,
    /// Members who stopped trading.
    dormant: BTreeSet<MemberId>,
    /// Keys handed out so far, so two arrivals never share one.
    arrivals: u32,
    /// A fractional carry of churn, so a rate below one member an epoch still
    /// retires somebody.
    churn_carry: f64,
}

impl Community {
    /// A founding community: `u` underwriters sharing `seed` equally, and
    /// nobody else — the founding members arrive by trade, like everyone.
    fn founded(seed: f64, u: u64, audit: Audit) -> Self {
        let mut st = State::default();
        let each = seed / u as f64;
        let underwriters = (0..u)
            .map(|i| st.add_underwriter(vec![key(i as usize)], each).expect("founding underwriter"))
            .collect();
        Community {
            d: Driver::new(st).audit_mode(audit),
            underwriters,
            members: Vec::new(),
            dormant: BTreeSet::new(),
            arrivals: 0,
            churn_carry: 0.0,
        }
    }

    fn key_of(&self, id: MemberId) -> Key {
        self.d.st.members[&id].keys[0]
    }

    /// The members still trading.
    fn live(&self) -> Vec<MemberId> {
        self.members.iter().copied().filter(|m| !self.dormant.contains(m)).collect()
    }

    /// Who can sponsor a newcomer right now: a live member the seed reaches
    /// with a work bond's headroom left — seating is bonded, never
    /// allowance-covered — or, when no member can, a founder, whose own supply
    /// is reach. What is measured is then the SEAT gate and not the sponsor's
    /// write budget, which decays with the backing behind it.
    fn sponsors(&self) -> Vec<MemberId> {
        let st = &self.d.st;
        let unit = st.params.bond_unit_minor();
        let able: Vec<MemberId> = self
            .live()
            .into_iter()
            .filter(|&m| edet_state::bond::established(st, m) && st.bond_headroom_minor(m) >= unit)
            .collect();
        if able.is_empty() {
            self.underwriters.clone()
        } else {
            able
        }
    }
}

/// Contracts to settle, cure or leave to default, by the epoch that acts.
type Due = BTreeMap<u64, Vec<(ContractId, bool)>>;

/// One obligation between two existing members, booked through the ordinary
/// transition. Returns the row and whether the community insured it.
fn accept(
    c: &mut Community,
    creditor: MemberId,
    debtor: MemberId,
    amount: f64,
    term: u64,
    m: &mut Measured,
) -> Option<(ContractId, bool)> {
    let id = c.d.st.next_contract;
    let signers = [c.key_of(creditor), c.key_of(debtor)];
    let out = c.d.apply(
        Tx::Accept {
            debtor: Party::Member(debtor),
            creditor: Party::Member(creditor),
            amount,
            maturity_epochs: term,
            arb: None,
        },
        &signers,
    );
    record(c, out, id, amount, m)
}

/// A newcomer's first trade: `sponsor` lends a key nobody holds, which seats
/// the row for the price of one seat on the sponsor's reach. Returns the
/// obligation, the new member's id and whether it was insured — it never is,
/// since a fresh row has nothing behind it.
fn arrive(
    c: &mut Community,
    sponsor: MemberId,
    amount: f64,
    term: u64,
    epoch: u64,
    m: &mut Measured,
) -> Option<(ContractId, MemberId)> {
    c.arrivals += 1;
    let k = fresh_key(usize::MAX, c.arrivals);
    let id = c.d.st.next_contract;
    let signers = [k, c.key_of(sponsor)];
    let out = c.d.apply(
        Tx::Accept {
            debtor: Party::Key(k),
            creditor: Party::Member(sponsor),
            amount,
            maturity_epochs: term,
            arb: None,
        },
        &signers,
    );
    if let Err(Error(ET_BOND_SEAT_UNBACKED)) = out {
        m.seat_refusals += 1;
        m.first_seat_refusal_epoch.get_or_insert(epoch);
        return None;
    }
    let booked = record(c, out, id, amount, m)?;
    let member = c.d.st.member_of_key(&k).expect("the trade seated the key");
    c.members.push(member);
    m.seats += 1;
    Some((booked.0, member))
}

/// Count one acceptance's outcome.
fn record(
    c: &Community,
    out: Result<(), Error>,
    id: ContractId,
    amount: f64,
    m: &mut Measured,
) -> Option<(ContractId, bool)> {
    match out {
        Ok(()) => {
            let insured = c.d.st.contracts[&id].insured;
            m.accepted_amount += amount;
            if insured {
                m.insured_amount += amount;
            }
            Some((id, insured))
        }
        Err(Error(ET_BOND_EXHAUSTED)) | Err(Error(ET_BOND_STATUS)) => {
            m.bond_refusals += 1;
            None
        }
        Err(_) => {
            m.other_refusals += 1;
            None
        }
    }
}

/// Discharge everything this epoch is holding, and record what defaults.
fn discharge_due(c: &mut Community, due: &mut Due, epoch: u64, p: &Process) {
    let acting = due.remove(&epoch).unwrap_or_default();
    for (id, on_time) in acting {
        let Some(row) = c.d.st.contracts.get(&id).cloned() else { continue };
        if !matches!(row.status, ContractStatus::Active | ContractStatus::Expired) {
            continue;
        }
        if !on_time && row.status == ContractStatus::Active {
            // Let it fall due: the epoch sweep expires it, and the cure lands
            // `cure_after` epochs later. A default that is never cured would
            // leave flow committed for the rest of the run, which is the
            // honest cost of one and what the peak measures.
            due.entry(epoch + p.cure_after).or_default().push((id, true));
            continue;
        }
        let amount = State::from_minor(row.outstanding);
        if amount <= c.d.st.params.dust {
            continue;
        }
        let tx = if row.status == ContractStatus::Expired {
            Tx::Cure { contract: id, amount }
        } else {
            Tx::Settle { contract: id, amount }
        };
        let signers = [c.key_of(row.debtor), c.key_of(row.creditor)];
        let _ = c.d.apply(tx, &signers);
    }
}

/// The ceremony, on schedule: one underwriter proposes an amendment for the
/// share of this epoch's rate headroom the founder set, and every other
/// underwriter assents. Enacted on the assent that crosses the threshold.
fn ceremony(c: &mut Community, epoch: u64, p: &Process, m: &mut Measured) {
    if p.amend_every == 0 || !epoch.is_multiple_of(p.amend_every) {
        return;
    }
    let headroom = edet_state::seed::headroom(&c.d.st);
    let amount = (headroom * p.amend_fraction * 100.0).floor() / 100.0;
    if amount <= c.d.st.params.dust {
        return;
    }
    let author = c.underwriters[((epoch / p.amend_every) % c.underwriters.len() as u64) as usize];
    let proposal = c.d.st.next_proposal;
    let signer = [c.key_of(author)];
    if c.d
        .apply(Tx::Propose { author, kind: ProposalKind::SeedAmendment { amount } }, &signer)
        .is_err()
    {
        return;
    }
    let before = c.d.st.external_seed();
    for &u in &c.underwriters {
        if u == author {
            continue;
        }
        let signer = [c.key_of(u)];
        let _ = c.d.apply(Tx::Assent { member: u, proposal }, &signer);
        if c.d.st.external_seed() > before {
            break;
        }
    }
    let after = c.d.st.external_seed();
    if after > before {
        m.ceremonies += 1;
        m.seed_added += after - before;
    }
}

/// Members who stop trading this epoch, at the founder's annual rate.
fn churn(c: &mut Community, rng: &mut Rng, p: &Process) {
    if p.churn_per_year == 0 {
        return;
    }
    let live = c.live();
    c.churn_carry += live.len() as f64 * p.churn_per_year as f64 / 100.0 / 365.0;
    while c.churn_carry >= 1.0 {
        c.churn_carry -= 1.0;
        if let Some(gone) = rng.pick(&live) {
            c.dormant.insert(gone);
        }
    }
}

/// Run the process against one candidate seed and report what it measured.
///
/// A warm-up year comes first, and it is not optional: standing is earned by
/// trading, so a community measured from genesis is measuring the bootstrap
/// rather than the steady state. The founding members arrive in the warm-up
/// through trades with the underwriters — the only edge a community with no
/// graph can write, and the seat each of them costs is the seat any row costs
/// — and every warm-up obligation runs from an underwriter to a member; in
/// the measured year they run between members, and newcomers arrive through
/// members who have standing to sponsor them.
fn run(seed: f64, p: &Process, audit: Audit) -> Measured {
    let u = p.underwriters;
    let mut c = Community::founded(seed, u, audit);
    let mut due: Due = BTreeMap::new();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut warm = Measured::default();

    let term = |rng: &mut Rng, p: &Process| -> u64 {
        let roll = rng.below(100);
        p.terms
            .iter()
            .find(|&&(_, cum)| roll < cum)
            .map(|&(t, _)| t)
            .expect("a term mix")
    };
    let amount = |rng: &mut Rng, p: &Process| -> f64 { ((p.mean_amount * (0.5 + rng.unit())) * 100.0).round() / 100.0 };

    // The warm-up. The founding members are seated by the underwriters as
    // fast as the write bond allows, and one epoch's trades per epoch run
    // from an underwriter to a member, settled on maturity: standing is what a
    // settlement leaves behind.
    for e in 1..=p.epochs {
        c.d.goto(e);
        discharge_due(&mut c, &mut due, e, p);
        ceremony(&mut c, e, p, &mut warm);
        while (c.members.len() as u64) < p.members {
            let sponsor = c.underwriters[c.members.len() % c.underwriters.len()];
            let t = term(&mut rng, p);
            let a = amount(&mut rng, p);
            let before = warm.bond_refusals + warm.seat_refusals;
            match arrive(&mut c, sponsor, a, t, e, &mut warm) {
                Some((id, _)) => due.entry(e + t).or_default().push((id, true)),
                None if warm.bond_refusals + warm.seat_refusals > before => break,
                None => {}
            }
        }
        let live = c.live();
        for _ in 0..p.trades_per_epoch {
            let Some(debtor) = rng.pick(&live) else { break };
            let creditor = c.underwriters[rng.below(u) as usize];
            let t = term(&mut rng, p);
            let a = amount(&mut rng, p);
            if let Some((id, _)) = accept(&mut c, creditor, debtor, a, t, &mut warm) {
                due.entry(e + t).or_default().push((id, true));
            }
        }
    }

    // The measured year.
    let mut measured = Measured { seats: warm.seats, ..Default::default() };
    let start = p.epochs;
    for e in start + 1..=start + p.epochs {
        c.d.goto(e);
        discharge_due(&mut c, &mut due, e, p);
        ceremony(&mut c, e, p, &mut measured);
        churn(&mut c, &mut rng, p);
        let sponsors = c.sponsors();
        for _ in 0..p.arrivals_per_epoch {
            let Some(sponsor) = rng.pick(&sponsors) else { break };
            let t = term(&mut rng, p);
            let a = amount(&mut rng, p);
            if let Some((id, _)) = arrive(&mut c, sponsor, a, t, e, &mut measured) {
                due.entry(e + t).or_default().push((id, true));
            }
        }
        let live = c.live();
        for _ in 0..p.trades_per_epoch {
            let (Some(creditor), Some(mut debtor)) = (rng.pick(&live), rng.pick(&live)) else { break };
            if debtor == creditor {
                debtor = live[(live.iter().position(|&m| m == creditor).unwrap_or(0) + 1) % live.len()];
            }
            if debtor == creditor {
                break;
            }
            let t = term(&mut rng, p);
            let a = amount(&mut rng, p);
            let on_time = rng.below(100) < p.settle_on_time;
            if let Some((id, _)) = accept(&mut c, creditor, debtor, a, t, &mut measured) {
                due.entry(e + t).or_default().push((id, on_time));
            }
        }
        let standing = c.d.st.committed_total();
        if standing > measured.peak_insured {
            measured.peak_insured = standing;
        }
    }
    measured.first_seat_refusal_epoch = measured.first_seat_refusal_epoch.or(warm.first_seat_refusal_epoch);
    measured.seat_refusals += warm.seat_refusals;
    measured.ceremonies += warm.ceremonies;
    measured.seed_added += warm.seed_added;
    measured.live_rows = c.live().len() as u64;
    measured.seed_final = c.d.st.external_seed();
    measured.seat_committed_final = State::from_minor(edet_kernel::flow::committed_total(&c.d.st.seat_committed));
    measured
}

/// The rehearsal. Four candidate seeds around the one named, so the answer is a
/// curve rather than a point — and the row where the insured share crosses 90%
/// is the one a founder is looking for, read beside the seats column.
#[test]
#[ignore]
fn the_seed_sizing_rehearsal() {
    let p = Process::from_env();
    let base = env_u64("EDET_SIZE_SEED", 10_000) as f64;
    let unit = State::default().params.bond_unit();
    let beta = State::default().params.seed_rate_bounded();
    println!(
        "\n{} underwriters, {} founding members, {} trades/epoch, mean {:.0}, terms {:?}, \
         {}% on time, cure after {}, {} epochs measured after a warm-up year of the same; \
         {} arrival(s)/epoch, {}% churn a year, a ceremony every {} epoch(s) at {:.0}% of the headroom",
        p.underwriters,
        p.members,
        p.trades_per_epoch,
        p.mean_amount,
        p.terms,
        p.settle_on_time,
        p.cure_after,
        p.epochs,
        p.arrivals_per_epoch,
        p.churn_per_year,
        p.amend_every,
        p.amend_fraction * 100.0
    );
    println!(
        "\n{:>10}  {:>13}  {:>9}  {:>8}  {:>8}  {:>14}  {:>10}  {:>10}  {:>10}",
        "seed", "peak insured", "insured", "bond", "other", "seats/ceiling", "1st refuse", "live rows", "seed final"
    );
    let mut crossing: Option<f64> = None;
    for factor in [0.5, 1.0, 2.0, 4.0] {
        let seed = base * factor;
        // **A rehearsal is a measurement, not an assertion**, and the audit is
        // an assertion: a year of trade at institutional size pays `1 + U + 2`
        // full cuts per epoch for a verdict this run does not read. The
        // ordinary probes below take the audited path, on a process small
        // enough that it costs nothing.
        let m = run(seed, &p, Audit::Off);
        let ceiling = (m.seed_final / unit).floor();
        println!(
            "{seed:>10.0}  {:>13.2}  {:>8.1}%  {:>8}  {:>8}  {:>6} / {:<6}  {:>10}  {:>10}  {:>10.0}",
            m.peak_insured,
            m.insured_share() * 100.0,
            m.bond_refusals,
            m.other_refusals,
            m.seats,
            ceiling,
            m.first_seat_refusal_epoch
                .map(|e| e.to_string())
                .unwrap_or_else(|| "never".into()),
            m.live_rows,
            m.seed_final,
        );
        if crossing.is_none() && m.insured_share() >= 0.90 {
            crossing = Some(seed);
        }
    }
    match crossing {
        Some(seed) => println!("\ninsured share crosses 90% at a seed of {seed:.0}"),
        None => println!("\ninsured share does not reach 90% at any seed measured — raise the range"),
    }
    println!(
        "size against PEAK INSURED, not against the year's volume, and err low: \
         a seed only ever falls by `DeclareSupply` and rises by a rate-bounded ceremony."
    );
    let need = p.members as f64 * unit;
    println!(
        "\na seat is one bond unit ({unit:.2}), so a seed carries seed/{unit:.0} rows and {} founding members \
         need a seed of at least {need:.0} before any credit is sized; the unit is `ParamKey::BondFraction` \
         in (0.001, 0.10), and moving it moves the write bond with it.",
        p.members
    );
    // Decay, in the unit a founder plans in. The ratio is a governed constant
    // with a safe range, and the sentence that matters is what a season does
    // to an edge nobody renews: at the genesis ratio a quarter keeps an
    // eighth of it and a year keeps two ten-thousandths. A community on a
    // seasonal rhythm sizes `StakeDecay` before it sizes the seed.
    let (num, den) = (edet_kernel::constants::STAKE_DECAY_NUM as f64, edet_kernel::constants::DECAY_DEN as f64);
    let r = num / den;
    println!(
        "\nan unrenewed stake halves in {:.0} epochs at the genesis ratio {num:.0}/{den:.0}, and keeps {:.1}% after a \
         quarter, {:.2}% after half a year and {:.3}% after a year; the reservation floor protects only what is still \
         owed, so decay is paid by the member who has repaid. `ParamKey::StakeDecay` in (900, 999) is the dial: at 999 \
         a year keeps {:.0}%.",
        (0.5f64).ln() / r.ln(),
        r.powi(90) * 100.0,
        r.powi(180) * 100.0,
        r.powi(365) * 100.0,
        (0.999f64).powi(365) * 100.0
    );
    let per_epoch = p.arrivals_per_epoch as f64 * unit;
    let yearly = |every: f64| (1.0 + beta).powf(365.0 / every);
    println!(
        "{} arrival(s) an epoch need {per_epoch:.2} of seed an epoch to keep pace — {:.2}% of the base seed per epoch \
         against a headroom of {:.1}% per ceremony that does not carry over: over a year the ceiling grows \
         {:.0}x on a ceremony a day, {:.0}% on one a month, {:.0}% on one a quarter.\n",
        p.arrivals_per_epoch,
        per_epoch / base * 100.0,
        beta * 100.0,
        yearly(1.0),
        (yearly(30.0) - 1.0) * 100.0,
        (yearly(91.0) - 1.0) * 100.0
    );
}

// ------------------------------------------------------------------ probes --

/// **Settlement is what turns the seed over.** The same seed carries more
/// insured trade in a year on 30-day terms than on 90-day ones, because the
/// flow an obligation reserves comes back at its settlement and not before.
///
/// Mutation that bites: stop discharging (`discharge_due` returning at the
/// top). Both terms then reserve the seed once and never release it, and the
/// two insured totals converge.
#[test]
fn settlement_turns_the_seed_over() {
    let mut short = Process::tiny();
    short.terms = vec![(30, 100)];
    let mut long = Process::tiny();
    long.terms = vec![(90, 100)];

    let quick = run(2_000.0, &short, Audit::EveryTick);
    let slow = run(2_000.0, &long, Audit::EveryTick);

    assert!(
        quick.insured_amount > slow.insured_amount * 1.5,
        "a 30-day term must turn the seed over faster than a 90-day one: \
         {:.2} insured against {:.2}",
        quick.insured_amount,
        slow.insured_amount
    );
}

/// **A seed below peak demand pushes rows to the uninsured tier**, which is
/// what understating it costs: the trade still happens, borne bilaterally.
///
/// The two assertions are one fact read twice. The supply arc is the ceiling,
/// so insured credit standing simultaneously can never exceed the seed; and a
/// process whose demand runs past that ceiling insures a smaller share of it
/// the smaller the seed is.
///
/// Mutation that bites: build `flow::reserve`'s network over a pristine
/// `Committed`, so a reservation stops charging the supply arcs it crosses.
/// It has to be `reserve` rather than `capacity` — `capacity` answers the
/// query and `reserve` places the hold, so a mutation to the reading alone
/// changes no book. The share inequality survives it, because halving the seed
/// still halves what the underwriters may confer and the stakes shrink with
/// it; the peak assertion is what catches it.
#[test]
fn a_seed_below_peak_demand_pushes_rows_uninsured() {
    let p = Process::tiny();
    let whole = run(2_000.0, &p, Audit::EveryTick);
    let half = run(1_000.0, &p, Audit::EveryTick);

    assert!(whole.insured_share() > 0.0, "the whole seed must insure something, or this probe measures nothing");
    assert!(
        whole.peak_insured <= 2_000.0 && half.peak_insured <= 1_000.0,
        "insured credit standing at once is bounded by the seed: {:.2} against 2,000 \
         and {:.2} against 1,000",
        whole.peak_insured,
        half.peak_insured
    );
    assert!(
        whole.peak_insured > 1_500.0,
        "this process must run the seed near its ceiling, or the bound above is vacuous: \
         {:.2} of 2,000",
        whole.peak_insured
    );
    assert!(
        half.insured_share() < whole.insured_share(),
        "halving the seed must push rows uninsured: {:.1}% insured at half against {:.1}% whole",
        half.insured_share() * 100.0,
        whole.insured_share() * 100.0
    );
}

/// **The seat ceiling binds at the seed over the unit, and a ceremony is what
/// moves it.** Two underwriters of 200 seat twenty rows and then nobody:
/// twelve founders in the warm-up, and arrivals until the live seats hold
/// the whole seed. With a ceremony every epoch admitting the whole rate
/// headroom, the seed grows and so does the count.
///
/// Mutation that bites: seat arrivals through `State::new_account` instead
/// of a trade naming their key. The rows appear, the seats hold nothing, and
/// `seat_committed_final` stays at zero against a seed of 400.
#[test]
fn the_seat_ceiling_binds_at_the_seed_over_the_unit() {
    let mut p = Process::tiny();
    p.arrivals_per_epoch = 2;
    let unit = State::default().params.bond_unit();
    let seed = 400.0;
    let ceiling = (seed / unit) as u64;

    let closed = run(seed, &p, Audit::EveryTick);
    assert!(closed.seats <= ceiling, "no community seats past its seed over the unit: {} of {ceiling}", closed.seats);
    assert_eq!(closed.seats, ceiling, "arrivals past the ceiling fill it exactly");
    assert_eq!(closed.seat_committed_final, seed, "and the live seats hold the whole seed");
    assert!(closed.seat_refusals > 0 && closed.first_seat_refusal_epoch.is_some(), "the gate refused the rest");

    let mut open = p.clone();
    open.amend_every = 1;
    let grown = run(seed, &open, Audit::EveryTick);
    assert!(grown.ceremonies > 0 && grown.seed_final > seed, "the ceremonies grew the seed: {grown:?}");
    assert!(
        grown.seats > ceiling,
        "and the grown seed seated more than the founding ceiling: {} against {ceiling}",
        grown.seats
    );
}
