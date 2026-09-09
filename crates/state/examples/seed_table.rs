//! Measure how many times a founding seed turns over in a year, per
//! settlement term, and emit the paper's §Adoption table.
//!
//! `cargo run -p edet-state --example seed_table`
//!
//! The sizing claim a founding community has to get right is that **the seed
//! is not consumed**: it is reserved for the life of an obligation, released
//! at settlement and reserved again. What it insures over a year is therefore
//! the seed multiplied by a turnover rate that settlement terms set, and the
//! table below is that rate measured rather than asserted.
//!
//! The process is the sentence made exact. One underwriter declares the seed;
//! one debtor starts with no standing, so their first trade is uninsured and
//! its settlement is what writes the stake (`discharge_credit`, capped by the
//! creditor's conferrable). From then on, every epoch, the debtor books the
//! largest amount their residual capacity insures at term `T` and settles
//! every obligation on its maturity epoch. The measured year begins one full
//! term after that first steady booking, so no term is credited with the
//! warm-up.
//!
//! **The shortest row is the maturity floor, not a round number.** A term
//! below `params.min_maturity_epochs` is refused at acceptance
//! (`ET-CTR-MATURITY-TOO-SHORT`) and no `ParamKey` reaches that field, so 30
//! epochs is the fastest settlement any chain admits and `365 / 30` is the
//! ceiling on turnover.
//!
//! What the measurement shows past the arithmetic is that decay costs the
//! turnover nothing: `flow::decay` floors every edge at its live reservation,
//! so the stake behind an outstanding obligation cannot fade under it, and the
//! settlement that releases the reservation re-stakes the edge in the same
//! epoch.

use edet_state::state::State;
use edet_state::tx::Tx;
use edet_state::types::{ContractId, ContractStatus, MemberId, Party};
use edet_swarm::driver::{Audit, Driver};
use edet_swarm::keys::member_key as key;

/// The seed one underwriter declares, in denomination units. A round figure so
/// the multiple column reads as a multiple.
const SEED: f64 = 10_000.0;
/// The measured year, in epochs. `EPOCH_SECS` is a day, so this is a year.
const YEAR: u64 = 365;
/// The settlement terms, in epochs. The first is the floor `min_maturity_epochs`
/// imposes; the last is a claim held for the whole year.
const TERMS: [u64; 5] = [30, 60, 90, 180, 365];

/// The measurement's own two moves over the tree's one driver: book at the
/// residual capacity, and settle in full on the maturity epoch.
///
/// `Audit::Off`, and deliberately: this emits a table the paper prints, the
/// process names only transitions the ledger admits, and every property the
/// audit would check is checked after every transition in the state suite. The
/// ids differ from the suite's and do not touch economics, which is what
/// `just seed-table-check` holds the output to.
fn accept(d: &mut Driver, creditor: MemberId, debtor: MemberId, amount: f64, term: u64) -> ContractId {
    let id = d.st.next_contract;
    d.ok(
        Tx::Accept {
            debtor: Party::Member(debtor),
            creditor: Party::Member(creditor),
            amount,
            maturity_epochs: term,
            arb: None,
        },
        &[key(creditor as usize), key(debtor as usize)],
    );
    id
}

fn settle(d: &mut Driver, contract: ContractId) {
    let c = d.st.contracts[&contract].clone();
    let amount = State::from_minor(c.outstanding);
    d.ok(Tx::Settle { contract, amount }, &[key(c.debtor as usize), key(c.creditor as usize)]);
}

/// One row: the insured trade a seed of `SEED` carries through a year at
/// settlement term `term`, and that trade as a multiple of the seed.
fn measure(term: u64) -> (f64, f64) {
    let mut r = Driver::new(State::default()).audit_mode(Audit::Off);
    let uw = r.st.add_underwriter(vec![key(0)], SEED).expect("founding underwriter");
    let debtor = r.st.new_account(vec![key(1)]);

    // The first trade is uninsured — nobody has staked on the debtor yet — and
    // its settlement is the only thing that can write the stake this whole
    // process runs on.
    let first = accept(&mut r, uw, debtor, SEED, term);
    r.goto(term);
    settle(&mut r, first);
    assert_eq!(r.st.contracts[&first].status, ContractStatus::Settled);
    assert!(r.st.capacity_of(debtor) > 0.0, "a settled trade must confer standing");

    // The first steady booking, outside the measured year: a term has to have
    // run once before the process is in the state the year measures.
    let mut open: Vec<ContractId> = Vec::new();
    let book = |r: &mut Driver, open: &mut Vec<ContractId>| -> f64 {
        let want = r.st.capacity_of(debtor);
        if want <= r.st.params.dust {
            return 0.0;
        }
        let id = accept(r, uw, debtor, want, term);
        assert!(r.st.contracts[&id].insured, "a booking at the residual capacity is insured");
        open.push(id);
        want
    };
    book(&mut r, &mut open);

    let start = r.st.epoch;
    let mut insured = 0.0;
    for e in start + 1..=start + YEAR {
        r.goto(e);
        let due: Vec<ContractId> = open.iter().copied().filter(|c| r.st.contracts[c].maturity_epoch == e).collect();
        for c in due {
            settle(&mut r, c);
            open.retain(|&o| o != c);
        }
        insured += book(&mut r, &mut open);
    }
    (insured, insured / SEED)
}

fn main() {
    println!("\\begin{{center}}");
    println!("\\begin{{tabular}}{{rrr}}");
    println!("\\toprule");
    println!("settlement term & insured trade per year & $\\times$ the seed \\\\");
    println!("\\midrule");
    for term in TERMS {
        let (trade, multiple) = measure(term);
        println!("${term}$ days & ${}$ & ${multiple:.1}\\times$ \\\\", grouped(trade));
    }
    println!("\\bottomrule");
    println!("\\end{{tabular}}");
    println!("\\end{{center}}");
}

/// A whole amount with LaTeX thin-space thousands separators, the way every
/// other figure in the paper is set.
fn grouped(x: f64) -> String {
    let digits = format!("{:.0}", x);
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push_str("{,}");
        }
        out.push(c);
    }
    out
}
