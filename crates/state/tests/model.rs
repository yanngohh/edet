//! The v0.7.0 model, driven through the transition function.
//!
//! `crates/kernel` proves the arithmetic. What is proved here is that the
//! state layer DRIVES it correctly: that acceptance reserves, that settlement
//! is the exact inverse, that a default holds its flow, that the two floors
//! bind, and that §Verification's invariants hold over sets after every transition.
//!
//! Every test ends by auditing the whole state. A property that holds in the
//! scene it was written for and breaks the ledger elsewhere is not a property.

mod common;

use common::{key, Chain, MATURITY, SUPPLY};
use edet_state::errors::*;
use edet_state::state::State;
use edet_state::tx::Tx;
use edet_state::types::*;

// ------------------------------------------------------------- the model --

/// A fresh key is worth exactly zero, and needs no rule to say so. Account
/// creation grants nothing, so there is nothing to approve.
#[test]
fn a_fresh_key_is_worth_nothing() {
    let c = Chain::founded(6, 2);
    assert_eq!(c.cap(6), 0.0);
    assert_eq!(c.st.capacity_of_set(&[6, 7]), 0.0);
}

/// The bootstrap, end to end: the first trade is UNINSURED (nobody has been
/// backed yet, so there is no flow to reserve), it settles, and the settlement
/// is what creates capacity. Capacity is the residue of trade, never its
/// precondition — which is why a community whose accounts all start at zero
/// does not deadlock.
#[test]
fn the_first_trade_is_uninsured_and_creates_the_standing_for_the_second() {
    let mut c = Chain::founded(1, 1);
    let (u, m) = (0, 1);
    let first = c.lend(u, m, 400.0);
    assert!(!c.st.contracts[&first].insured, "nothing backs the newcomer yet, so nothing is reserved");
    assert_eq!(c.st.committed_total(), 0.0);

    c.settle(first, 400.0);
    assert_eq!(c.cap(m), 400.0, "the settlement is what conferred standing");

    // And now the same trade is insured, because there is flow to reserve.
    let second = c.lend(u, m, 400.0);
    assert!(c.st.contracts[&second].insured);
    assert_eq!(c.st.committed_total(), 400.0, "insured credit is the flow drawn through the underwriters");
    assert_eq!(c.cap(m), 0.0, "and it is held: a unit of standing backs one obligation at a time");
}

/// Standing runs creditor → debtor, so it is evidence of having OWED and paid.
/// A member who only ever delivers accumulates none of it, however much they
/// deliver and to however many people who have it.
#[test]
fn selling_confers_nothing_on_the_seller() {
    let mut c = Chain::founded(1, 3);
    let (u, buyer, seller) = (0, 1, 2);
    // Give the buyer real standing first.
    let bootstrap = c.lend(u, buyer, SUPPLY);
    c.settle(bootstrap, SUPPLY);
    assert_eq!(c.cap(buyer), SUPPLY);

    // Now the seller delivers, over and over, to somebody well backed.
    for _ in 0..20 {
        let sale = c.lend(seller, buyer, 100.0);
        c.settle(sale, 100.0);
    }
    assert_eq!(c.cap(seller), 0.0, "2000 of settled sales are worth nothing to the seller");

    // One honoured purchase, and the picture changes at once.
    let purchase = c.lend(buyer, seller, 100.0);
    c.settle(purchase, 100.0);
    assert!(c.cap(seller) > 0.0, "one honoured debt confers standing immediately");
}

/// Wash trading confers nothing, and there is nothing to detect: an edge is
/// capped by what the creditor may confer, so between two accounts that may
/// confer nothing every settlement stakes exactly zero.
#[test]
fn wash_trading_creates_nothing() {
    let mut c = Chain::founded(1, 2);
    let (u, a, b) = (0, 1, 2);
    for who in [a, b] {
        let boot = c.lend(u, who, SUPPLY);
        c.settle(boot, SUPPLY);
    }
    let before = c.st.gross_capacity_of_set(&[a, b]);
    assert_eq!(before, SUPPLY, "both are fully backed, by one supply");

    // Now trade between themselves, as hard as they can, forever.
    for _ in 0..20 {
        let x = c.lend(a, b, 1000.0);
        c.settle(x, 1000.0);
        let y = c.lend(b, a, 1000.0);
        c.settle(y, 1000.0);
    }
    assert!(c.st.edges.contains_key(&(a as usize, b as usize)), "the internal edges are real");
    assert_eq!(
        c.st.gross_capacity_of_set(&[a, b]),
        before,
        "and worth exactly nothing: internal edges never cross the set's own boundary"
    );
    // Individually too — the peak rule means repeating a cycle raises nothing.
    assert_eq!(c.cap(a), SUPPLY);
    assert_eq!(c.cap(b), SUPPLY);
}

/// **The set form, which is the security statement.** One underwriter backing
/// k debtors lends ONE supply in total, however many of them there are — and
/// this is the exact scene a per-account check passes in every time.
#[test]
fn one_supply_is_lent_once_however_many_debtors_share_it() {
    for k in [2usize, 5, 20] {
        let mut c = Chain::founded(1, k);
        let debtors: Vec<MemberId> = (1..=k as MemberId).collect();
        // Back each of them to the hilt, through a settled first trade.
        for &d in &debtors {
            let boot = c.lend(0, d, SUPPLY);
            c.settle(boot, SUPPLY);
            assert_eq!(c.cap(d), SUPPLY, "each of them individually looks fully backed");
        }
        assert_eq!(c.st.capacity_of_set(&debtors), SUPPLY, "k={k}: but together, one supply");

        // Now draw. The total insured must be one supply, not k of them.
        for &d in &debtors {
            c.lend(0, d, SUPPLY);
        }
        assert_eq!(c.st.committed_total(), SUPPLY, "k={k}: one supply, lent once");
    }
}

/// Settlement gives back exactly what acceptance took — including the upstream
/// half of a multi-hop path. Crediting only the arcs incident to the debtor
/// stranded those forever, so settling an obligation destroyed capacity that
/// nothing was behind.
#[test]
fn settling_a_multi_hop_obligation_gives_the_whole_path_back() {
    let mut c = Chain::founded(1, 2);
    let (u, m1, m2) = (0, 1, 2);
    let a = c.lend(u, m1, SUPPLY);
    c.settle(a, SUPPLY);
    let b = c.lend(m1, m2, SUPPLY);
    c.settle(b, SUPPLY);
    assert_eq!(c.cap(m2), SUPPLY, "standing propagates undiminished");

    let deep = c.lend(u, m2, SUPPLY);
    assert!(c.st.contracts[&deep].insured);
    assert_eq!(c.st.contracts[&deep].held.edges.len(), 2, "both hops are held");
    assert_eq!(c.cap(m2), 0.0);

    c.settle(deep, SUPPLY);
    assert_eq!(c.st.reserved_total(), 0.0, "and both hops come back");
    assert_eq!(c.st.committed_total(), 0.0);
    assert_eq!(c.cap(m2), SUPPLY, "settling must not destroy capacity");
}

/// Partial settlement re-holds exactly what is still owed. The old obligation
/// is released whole and a fresh one taken for the remainder, so `Σ committed`
/// tracks the book exactly rather than approximately.
#[test]
fn partial_settlement_holds_exactly_what_is_still_owed() {
    let mut c = Chain::founded(1, 1);
    let (u, m) = (0, 1);
    let boot = c.lend(u, m, SUPPLY);
    c.settle(boot, SUPPLY);

    let live = c.lend(u, m, 2000.0);
    assert_eq!(c.st.committed_total(), 2000.0);
    c.settle(live, 500.0);
    assert_eq!(c.st.committed_total(), 1500.0, "exactly the remainder, not a share of the original");
    assert_eq!(c.cap(m), 1000.0);
    c.settle(live, 1500.0);
    assert_eq!(c.st.committed_total(), 0.0);
    assert_eq!(c.cap(m), SUPPLY);
}

/// **A default does not release the flow it committed.** That one line is the
/// whole sanction: the defaulter's standing stays consumed by the debt they
/// did not pay, so stealing costs the thief exactly what they hold, once, and
/// cannot be repeated.
#[test]
fn a_default_does_not_give_the_flow_back() {
    let mut c = Chain::founded(1, 1);
    let (u, m) = (0, 1);
    let boot = c.lend(u, m, SUPPLY);
    c.settle(boot, SUPPLY);
    let bad = c.lend(u, m, SUPPLY);
    assert_eq!(c.st.committed_total(), SUPPLY);

    c.default_on(bad);
    assert_eq!(c.st.contracts[&bad].status, ContractStatus::Expired);
    assert_eq!(c.st.committed_total(), SUPPLY, "the flow a defaulter took stays taken");
    assert_eq!(c.cap(m), 0.0, "so their capacity does not come back, and they cannot do it again");

    // Curing is the only way out, and it releases in step with what is paid.
    c.ok(Tx::Cure { contract: bad, amount: SUPPLY }, &[key(u as usize), key(m as usize)]);
    assert_eq!(c.st.committed_total(), 0.0);
    assert_eq!(c.cap(m), SUPPLY);
}

// ------------------------------------------------------------ the floors --

/// **A declaration cannot be raised at all**, whatever the declarer's own
/// capacity. A supply is seated by a ceremony — genesis, or a §Governance
/// amendment — and this transition may only lower one.
///
/// Capping it by the declarer's CAPACITY has the argument that the
/// cap is what keeps a declared supply from being a free signature: an account
/// nobody backs has capacity zero and may declare exactly zero. True, and not
/// enough. Capacity is what the community conferred, so a declaration against
/// it is a promise made on the strength of another promise — and the
/// what a coalition does with a chain of them is 204,800 of ledger-labelled
/// INSURED credit issued to sybils it controls, against an external seed of
/// 100. So the cap is a refusal instead.
#[test]
fn a_declaration_cannot_be_raised_by_any_amount_of_capacity() {
    let mut c = Chain::founded(1, 1);
    let (u, m) = (0, 1);
    // Refused TWICE over, and the outer refusal is the older one: a
    // supply raise is bonded, so a key nobody backs cannot even pay to ask.
    assert_eq!(
        c.apply(Tx::DeclareSupply { member: m, supply: 1.0 }, &[key(m as usize)]),
        Err(Error(ET_BOND_EXHAUSTED)),
        "zero is absorbing: a key nobody backs cannot even afford to declare"
    );

    // And on its own merits, once it can afford to ask: backed above the
    // establishment floor, so the write gate admits the question and the
    // declaration itself is what refuses it.
    let seed = c.lend(u, m, c.st.params.v_base * 0.1);
    let amount = c.outstanding(seed);
    c.settle(seed, amount);
    assert_eq!(
        c.apply(Tx::DeclareSupply { member: m, supply: 1000.0 }, &[key(m as usize)]),
        Err(Error(ET_UWR_ABOVE_CAPACITY)),
        "a declaration above what the community has put behind you is refused on its merits"
    );

    // And with the capacity in hand, which is the half that changed: the
    // community has put 1000 behind them, and it buys not one unit of supply.
    let boot = c.lend(u, m, 1000.0);
    c.settle(boot, 1000.0);
    assert_eq!(c.cap(m), 1000.0);
    for want in [1000.01, 1000.0, 1.0] {
        assert_eq!(
            c.apply(Tx::DeclareSupply { member: m, supply: want }, &[key(m as usize)]),
            Err(Error(ET_UWR_ABOVE_CAPACITY)),
            "a raise to {want} against a capacity of 1000 is still a raise"
        );
    }
    assert_eq!(c.st.underwriters.get(&m), None, "and they are not an underwriter");
    assert_eq!(c.st.conferrable(m), 1000.0, "what they may confer is their capacity, which is not a supply");
    // The founder, who IS one, confers the supply the ceremony seated rather
    // than the capacity nobody has given them.
    assert_eq!(c.cap(u), 0.0);
    assert_eq!(c.st.conferrable(u), SUPPLY, "an underwriter confers their supply, not their capacity");
}

/// A withdrawal is floored at the flow committed through it (§Stability). Without the
/// floor, one of six underwriters leaving a fully drawn community left
/// capacity 12,500 against 15,000 outstanding — the cut bound broken outright.
#[test]
fn a_supply_cannot_be_withdrawn_under_live_credit() {
    let mut c = Chain::founded(1, 1);
    let (u, m) = (0, 1);
    let boot = c.lend(u, m, SUPPLY);
    c.settle(boot, SUPPLY);
    c.lend(u, m, 2000.0);
    assert_eq!(c.st.supply_floor(u), 200_000, "2000 stands on this underwriter");

    assert_eq!(
        c.apply(Tx::DeclareSupply { member: u, supply: 1999.0 }, &[key(u as usize)]),
        Err(Error(ET_UWR_BELOW_COMMITTED)),
        "the debt did not shrink because the underwriter changed their mind"
    );
    // Down to the floor is legal, and the bound survives it.
    c.ok(Tx::DeclareSupply { member: u, supply: 2000.0 }, &[key(u as usize)]);
    assert_eq!(c.st.committed_total(), 2000.0);
    assert!(c.st.committed_total() <= c.st.capacity_of_set(&[m]) + 2000.0);
}

/// Decay never releases live collateral: the debt did not shrink because the
/// evidence aged, so the collateral behind it may not either.
#[test]
fn decay_cannot_release_live_collateral() {
    let mut c = Chain::founded(1, 1);
    let (u, m) = (0, 1);
    let boot = c.lend(u, m, SUPPLY);
    c.settle(boot, SUPPLY);
    c.lend(u, m, 2000.0);

    for e in 1..25 {
        c.goto(e);
        assert_eq!(c.st.committed_total(), 2000.0, "an outstanding obligation keeps its collateral");
    }
    assert_eq!(c.st.edges[&(u as usize, m as usize)], 200_000, "the stake is floored at its reservation");
}

/// **One ledger already hosts many communities, and needs nothing to do it.**
///
/// There is no `Community` object anywhere in the state — only underwriters,
/// stakes and accounts. A "community" is a region of the graph, and the
/// boundary between two of them is simply the absence of a path. So two
/// co-ops sharing one chain stay separate by arithmetic: no stake edge joins
/// them, so no flow crosses, so neither underwrites the other.
///
/// This is why §Model's boundary is about the TOTAL ORDER and not about the graph,
/// and it is worth stating as a measurement because it decides how much
/// machinery the boundary question actually needs: two communities that expect
/// to trade can simply share a chain, and then there is nothing to bridge and
/// nothing to merge.
#[test]
fn one_chain_hosts_many_communities_with_no_machinery_at_all() {
    let mut c = Chain::founded(2, 6);
    // Underwriter 0 backs one co-op, underwriter 1 backs the other. Nobody
    // trades across, so no edge ever joins them.
    let bakers: Vec<MemberId> = vec![2, 3, 4];
    let library: Vec<MemberId> = vec![5, 6, 7];
    for &m in &bakers {
        let boot = c.lend(0, m, SUPPLY);
        c.settle(boot, SUPPLY);
    }
    for &m in &library {
        let boot = c.lend(1, m, SUPPLY);
        c.settle(boot, SUPPLY);
    }

    // Each draws on its own underwriter and on nobody else's — not by a rule,
    // but because there is no path to the other one.
    assert_eq!(c.st.gross_capacity_of_set(&bakers), SUPPLY);
    assert_eq!(c.st.gross_capacity_of_set(&library), SUPPLY);

    // And the two together are bounded by the two supplies. A coalition
    // spanning BOTH is measured as one set on one chain, which is strictly
    // stronger than what two separate chains can say about each other — a cut
    // over one chain's accounts cannot see a coalition spanning two.
    let both: Vec<MemberId> = bakers.iter().chain(library.iter()).copied().collect();
    assert_eq!(c.st.gross_capacity_of_set(&both), 2.0 * SUPPLY);

    // The moment somebody trades across, the graph joins them — which is what
    // "an economy is emergent" means, and it needs no transition of its own.
    let bridge = c.lend(bakers[0], library[0], 400.0);
    c.settle(bridge, 400.0);
    assert!(
        c.st.gross_capacity_of_set(&library) > SUPPLY,
        "the bakers' underwriter now reaches the library, because a path exists"
    );
}

// ------------------------------------------------------------- the spam --

/// **The spam floor is zero.** A ring of free keys trading only with each
/// other writes nothing: no signer has anything to lose, so the bond gate
/// refuses. A per-key free allowance would be the free-signature bound's own
/// defect one layer down — keys are free, so N keys would carry N allowances.
#[test]
fn a_ring_of_free_keys_cannot_write() {
    let mut c = Chain::founded(1, 2);
    let (a, b) = (1, 2);
    assert_eq!(
        c.apply(
            Tx::Accept {
                debtor: Party::Member(a),
                creditor: Party::Member(b),
                amount: 10.0,
                maturity_epochs: MATURITY,
                arb: None
            },
            &[key(1), key(2)],
        ),
        Err(Error(ET_BOND_EXHAUSTED)),
        "two accounts nobody has backed may not fill the ledger for free"
    );
    assert!(c.st.contracts.is_empty());
}

/// But the newcomer's genuine first trade still goes through, billed to the
/// established counterparty who chose to trade with them — the same cost §Recourse
/// already names, priced at one refundable bond instead of nothing.
#[test]
fn an_established_member_carries_a_newcomer() {
    let mut c = Chain::founded(1, 1);
    let first = c.lend(0, 1, 400.0);
    assert!(!c.st.contracts[&first].insured, "uninsured, as it must be");
    c.settle(first, 400.0);
    assert_eq!(c.cap(1), 400.0);
}

/// **The audit's own probe: invariant 1 must FIRE, and only the set catches
/// it.**
///
/// Every other test in this tree asserts that `audit` PASSES. That is a claim
/// about legal states and says nothing about what the audit detects — an
/// invariant that always returned `Ok` would keep every one of them green. This
/// is the other direction, and it is built by hand because the transition
/// function cannot reach it: `reserve_capacity` is what makes the honest states
/// honest, so a violation has to be written directly into stored state, the way
/// a defect in the transition function would write it.
///
/// The scene is the one §Verification says the family exists for. One underwriter's
/// supply funnels through a single intermediary, and two obligations each claim
/// a private final arc while both actually draw on the shared one:
///
/// ```text
///   source --1000--> u --500--> x --500--> d1     each holding 300 of u
///                                --500--> d2
/// ```
///
/// Every singleton passes: the cut into `d1` alone is 500 against 300 drawn.
/// So do invariants 2, 4 and 5 — each reservation is inside its own edge, the
/// committed total is inside the supply, and the book agrees with the cache.
/// The union is what fails: 600 drawn against a cut of 500, because the two
/// obligations were sized against the same 500 twice.
///
/// This also pins the two clauses of `drawn` that are load-bearing. Only supply
/// reaching the set from OUTSIDE counts, and the sum is over every obligation
/// of every account inside — which is the part that indexes
/// the book once instead of re-scanning it per set, and a rewrite whose only
/// gate was "the legal states still pass" would not have been gated at all.
#[test]
fn the_cut_bound_fires_over_a_set_that_every_singleton_clears() {
    use edet_kernel::flow;
    use edet_state::invariants::audit;
    use edet_state::state::State;

    let mut st = State::default();
    let u = st.add_underwriter(vec![key(0)], 10.0).expect("the underwriter");
    let x = st.new_account(vec![key(1)]);
    let d1 = st.new_account(vec![key(2)]);
    let d2 = st.new_account(vec![key(3)]);
    let creditor = st.new_account(vec![key(4)]);

    // 10.00 of supply, funnelled through `x` by a 5.00 arc.
    st.edges.insert((u as usize, x as usize), 500);
    st.edges.insert((x as usize, d1 as usize), 500);
    st.edges.insert((x as usize, d2 as usize), 500);

    // Two obligations of 3.00, each recording only its own final arc — so
    // nothing in stored state says the 5.00 into `x` was spent twice.
    let mut book = |debtor: MemberId, last: (usize, usize)| {
        let id = st.next_contract;
        st.next_contract += 1;
        let held = flow::Held { edges: vec![(last, 300)], supply: vec![(u as usize, 300)] };
        st.contracts.insert(
            id,
            Contract {
                id,
                debtor,
                creditor,
                outstanding: State::to_minor(3.0),
                original: State::to_minor(3.0),
                maturity_epoch: st.epoch + MATURITY,
                status: ContractStatus::Active,
                created_epoch: st.epoch,
                accepted_epoch: st.epoch,
                insured: true,
                held,
                arb: None,
                arb_attestations: Default::default(),
                arb_awarded: false,
            },
        );
        st.members.get_mut(&debtor).expect("debtor").debt_out += State::to_minor(3.0);
        *st.reserved.entry(last).or_insert(0) += 300;
        *st.committed.entry(u as usize).or_insert(0) += 300;
    };
    book(d1, (x as usize, d1 as usize));
    book(d2, (x as usize, d2 as usize));

    // Each debtor on its own is comfortably inside its cut, which is why a
    // per-account check passes here and why this scene exists.
    for d in [d1, d2] {
        assert_eq!(st.gross_capacity_of_set(&[d]), 5.0, "the cut into one debtor is the whole funnel");
        assert!(3.0 <= st.gross_capacity_of_set(&[d]), "and 3.00 fits inside it");
    }
    // And the funnel is what the pair share.
    assert_eq!(st.gross_capacity_of_set(&[d1, d2]), 5.0, "two debtors behind one 5.00 arc still share 5.00");

    let v = audit(&st).expect_err("the audit must reject a set drawing 6.00 through a cut of 5.00");
    assert!(v.0.starts_with("invariant 1:"), "it must fail as the CUT BOUND, not as something upstream: {}", v.0);
    assert!(v.0.contains("holds 600 against a cut of 500"), "and it must report both quantities: {}", v.0);

    // Repaying one of them to 2.00 brings the pair back inside the funnel, so
    // the audit is measuring the quantity and not merely refusing the shape.
    let id = *st.contracts.keys().next().expect("a contract");
    let c = st.contracts.get_mut(&id).expect("a contract");
    c.outstanding = State::to_minor(2.0);
    c.held = flow::Held { edges: vec![(c.held.edges[0].0, 200)], supply: vec![(u as usize, 200)] };
    let last = st.contracts[&id].held.edges[0].0;
    st.reserved.insert(last, 200);
    st.committed.insert(u as usize, 500);
    st.members.get_mut(&st.contracts[&id].debtor.clone()).expect("debtor").debt_out = State::to_minor(2.0);
    audit(&st).expect("5.00 drawn through a cut of 5.00 is legal, and the bound is not strict");
}

/// **And the other load-bearing clause of invariant 1, which nothing checked.**
///
/// §Verification names two details as load-bearing and says each was got wrong once. The
/// pristine residual has a probe (`denomination.rs`, the fully drawn community).
/// The second — *only supply reaching the set from OUTSIDE it counts on the
/// left* — had none: deleting the clause outright leaves every test in this
/// workspace green, because in every other scene the underwriters are not
/// themselves debtors, so there is never any inside supply to wrongly count.
/// A clause that cannot fail in any fixture is not gated by any of them.
///
/// Here `u2` is both. It underwrites `d1` and owes 3.00 of its own to `u1`, so
/// the set of live insured debtors contains an underwriter:
///
/// ```text
///   source --10.00--> u1 --5.00--> u2 --5.00--> d1
///   source --10.00--> u2  (excluded: u2 is inside the measured set)
/// ```
///
/// A cut over `{u2, d1}` does not measure `u2`'s own supply — `Network::build`
/// drops the source arc of any underwriter inside the target set, which is the
/// same clause on the other side, and is what stops a coalition underwriting
/// itself. So the left-hand side must drop it too: 3.00 reaching the set from
/// `u1`, against a funnel of 5.00. Counting `u2`'s own 3.00 as well reads 6.00
/// against that same 5.00 and halts a ledger with nothing wrong with it —
/// comparing two different quantities and failing by accident, which is the
/// mirror of §Verification's warning that it would otherwise PASS by accident.
#[test]
fn a_cut_does_not_count_the_supply_of_an_underwriter_inside_the_set() {
    use edet_kernel::flow;
    use edet_state::invariants::audit;
    use edet_state::state::State;

    let mut st = State::default();
    let u1 = st.add_underwriter(vec![key(0)], 10.0).expect("the outside underwriter");
    let u2 = st.add_underwriter(vec![key(1)], 10.0).expect("the underwriter who also owes");
    let d1 = st.new_account(vec![key(2)]);
    let creditor = st.new_account(vec![key(3)]);

    st.edges.insert((u1 as usize, u2 as usize), 500);
    st.edges.insert((u2 as usize, d1 as usize), 500);

    let mut book = |debtor: MemberId, through: MemberId, arc: (usize, usize)| {
        let id = st.next_contract;
        st.next_contract += 1;
        st.contracts.insert(
            id,
            Contract {
                id,
                debtor,
                creditor,
                outstanding: State::to_minor(3.0),
                original: State::to_minor(3.0),
                maturity_epoch: st.epoch + MATURITY,
                status: ContractStatus::Active,
                created_epoch: st.epoch,
                accepted_epoch: st.epoch,
                insured: true,
                held: flow::Held { edges: vec![(arc, 300)], supply: vec![(through as usize, 300)] },
                arb: None,
                arb_attestations: Default::default(),
                arb_awarded: false,
            },
        );
        st.members.get_mut(&debtor).expect("debtor").debt_out += State::to_minor(3.0);
        *st.reserved.entry(arc).or_insert(0) += 300;
        *st.committed.entry(through as usize).or_insert(0) += 300;
    };
    // `u2` owes through `u1`; `d1` owes through `u2`.
    book(u2, u1, (u1 as usize, u2 as usize));
    book(d1, u2, (u2 as usize, d1 as usize));

    // The set of live insured debtors is {u2, d1}, and its cut is the 5.00
    // funnel from `u1` — `u2`'s own 10.00 does not underwrite a set it is in.
    assert_eq!(
        st.gross_capacity_of_set(&[u2, d1]),
        5.0,
        "an underwriter inside the set must not supply it, or the coalition underwrites itself"
    );

    // 3.00 reaches that set from outside it. The other 3.00 comes from within,
    // and counting it would read 6.00 against 5.00 and halt this ledger.
    audit(&st).expect("a legal ledger where an underwriter is also a debtor must audit clean");
}

// ------------------------------------------------ the audit's own cost --

/// The scene `the_cut_bound_fires_over_a_set_that_every_singleton_clears`
/// builds, stopped one step short of illegal: one 10.00 supply funnelled
/// through `x` by a 5.00 arc, two debtors behind it drawing 2.50 each. Drawn
/// 500 against a cut of 500 — legal, and exactly at the bound, which is where
/// a cached lower bound is worth testing.
fn funnelled_at_the_bound() -> (edet_state::state::State, MemberId, MemberId, MemberId) {
    use edet_kernel::flow;
    use edet_state::state::State;

    let mut st = State::default();
    let u = st.add_underwriter(vec![key(0)], 10.0).expect("the underwriter");
    let x = st.new_account(vec![key(1)]);
    let d1 = st.new_account(vec![key(2)]);
    let d2 = st.new_account(vec![key(3)]);
    let creditor = st.new_account(vec![key(4)]);

    st.edges.insert((u as usize, x as usize), 500);
    st.edges.insert((x as usize, d1 as usize), 500);
    st.edges.insert((x as usize, d2 as usize), 500);

    for (debtor, last) in [(d1, (x as usize, d1 as usize)), (d2, (x as usize, d2 as usize))] {
        let id = st.next_contract;
        st.next_contract += 1;
        let held = flow::Held { edges: vec![(last, 250)], supply: vec![(u as usize, 250)] };
        st.contracts.insert(
            id,
            Contract {
                id,
                debtor,
                creditor,
                outstanding: State::to_minor(2.5),
                original: State::to_minor(2.5),
                maturity_epoch: st.epoch + MATURITY,
                status: ContractStatus::Active,
                created_epoch: st.epoch,
                accepted_epoch: st.epoch,
                insured: true,
                held,
                arb: None,
                arb_attestations: Default::default(),
                arb_awarded: false,
            },
        );
        st.members.get_mut(&debtor).expect("debtor").debt_out += State::to_minor(2.5);
        *st.reserved.entry(last).or_insert(0) += 250;
        *st.committed.entry(u as usize).or_insert(0) += 250;
    }
    assert_eq!(st.gross_capacity_of_set(&[d1, d2]), 5.0);
    (st, u, x, d1)
}

/// **A block that changed nothing costs nothing**, which is the whole of the
/// complaint about §Implementation and the reason the cache exists.
///
/// The audit ran `1 + U + 2` full max-flow queries after every committed block
/// "whatever the block contained, including nothing" — 5.17 s at 20,000
/// accounts with 200 underwriters, against a default consensus round of 5 s.
/// A cut is monotone in the three things it reads and reads nothing else, so a
/// cut already verified stays a valid lower bound until something decreases,
/// and re-auditing an unchanged state is dominated work rather than a check.
#[test]
fn a_repeated_audit_over_unchanged_state_computes_nothing() {
    use edet_state::invariants::{audit_with_cache, AuditCache};

    let (st, ..) = funnelled_at_the_bound();
    let mut cache = AuditCache::default();

    audit_with_cache(&st, &mut cache).expect("the scene is legal");
    let cold = cache.queries();
    assert!(cold > 0, "the first audit has nothing to lean on and must do the work: {cold}");

    for _ in 0..16 {
        audit_with_cache(&st, &mut cache).expect("still legal, and still the same state");
    }
    assert_eq!(cache.queries(), cold, "sixteen audits of an unchanged state must compute no cut at all");
}

/// **And the guard that makes that sound: a cut input that FALLS throws the
/// cached value away.**
///
/// This is the probe the whole design rests on. The cached number is a lower
/// bound only while nothing it was measured on has decreased; drop the guard
/// and the memo answers from a graph that no longer exists. Here the funnel is
/// narrowed from 5.00 to 4.00 behind the audit's back — which is exactly the
/// defective transition an audit exists to catch — while the 5.00 already
/// drawn through it stands. Invariants 2, 4 and 5 stay clean, so nothing
/// upstream of the cut bound can catch it either.
#[test]
fn a_lowered_edge_is_not_answered_from_the_memo() {
    use edet_state::invariants::{audit, audit_with_cache, AuditCache};

    let (mut st, u, x, _) = funnelled_at_the_bound();
    let mut cache = AuditCache::default();
    audit_with_cache(&st, &mut cache).expect("legal at the bound");
    let before = cache.queries();

    // The funnel narrows. Nothing else moves: the obligations still hold
    // 250 each on their own final arcs, so 2, 4 and 5 are untouched.
    st.edges.insert((u as usize, x as usize), 400);
    assert_eq!(st.gross_capacity_of_set(&[2, 3]), 4.0, "the cut has fallen to 4.00 under 5.00 of drawn credit");

    let cold = audit(&st).expect_err("the cold audit must reject 500 drawn against a cut of 400");
    assert!(cold.0.starts_with("invariant 1:"), "as the cut bound: {}", cold.0);

    let warm = audit_with_cache(&st, &mut cache).expect_err("and the cached audit must reject it too");
    assert!(warm.0.starts_with("invariant 1:"), "as the same thing: {}", warm.0);
    assert!(cache.queries() > before, "it must have RECOMPUTED rather than answered from a stale bound");
}

/// The other direction, and the cheap half working as intended: nothing
/// decreased, so the memo stands — but what is drawn has risen past it, and a
/// bound that no longer covers the draw is not a proof. It costs a real query
/// and the audit fires.
#[test]
fn a_risen_draw_past_a_standing_bound_still_costs_a_query() {
    use edet_kernel::flow;
    use edet_state::invariants::{audit_with_cache, AuditCache};

    let (mut st, u, _, d1) = funnelled_at_the_bound();
    let mut cache = AuditCache::default();
    audit_with_cache(&st, &mut cache).expect("legal at the bound");
    let before = cache.queries();

    // One debtor draws 0.50 more. No edge and no supply moved, so the guard
    // is satisfied and every cached cut is still valid — and still 500.
    let id = *st
        .contracts
        .iter()
        .find(|(_, c)| c.debtor == d1)
        .map(|(id, _)| id)
        .expect("the first debtor's obligation");
    let c = st.contracts.get_mut(&id).expect("a contract");
    let last = c.held.edges[0].0;
    c.outstanding = State::to_minor(3.0);
    c.held = flow::Held { edges: vec![(last, 300)], supply: vec![(u as usize, 300)] };
    st.members.get_mut(&d1).expect("debtor").debt_out = State::to_minor(3.0);
    *st.reserved.get_mut(&last).expect("the arc") = 300;
    *st.committed.get_mut(&(u as usize)).expect("the supply arc") = 550;

    let warm = audit_with_cache(&st, &mut cache).expect_err("550 drawn against a cut of 500");
    assert!(warm.0.starts_with("invariant 1:"), "as the cut bound: {}", warm.0);
    assert!(cache.queries() > before, "the bound did not cover the draw, so it had to be measured");
}

// ------------------------------------- the two modelling questions, closed --

/// **Priority under a binding ceiling: first-come-first-served is the rule,
/// and what it actually rations is measured here rather than described.**
///
/// The open question was who should be insured when utilisation is pinned.
/// The decision is that nothing is added, and this probe is why: what a
/// binding ceiling rations is not credit but INSURANCE, and it rations it in
/// whole rows, because `insured` is decided per obligation and may only ever
/// fall. So with 2.00 of a supply arc left, one row of 3.00 insures NOTHING
/// while three rows of 1.00 insure 2.00 of the same trade — the implicit rule
/// is first-come, and smaller first if you split.
///
/// That asymmetry is real and it is recorded rather than fixed. A priority
/// rule would be a second answer to "how much may this member owe", which is
/// the shape §Standing's macroprudential brake is refused for; and the three ways to
/// build one all cost more than the artefact: a queue cannot hold a place
/// (an unreserved pledge is exactly what the loss pool was retired for), a
/// reservation that depends on who is asking stops capacity being one number
/// per account and forces the free-signature bound to be restated per class, and a
/// ranking key has to be forge-resistant precisely where a forged one pays.
/// The one key the model already makes expensive is standing, and
/// priority-by-standing serves the best-backed member first.
#[test]
fn a_binding_ceiling_rations_insurance_in_whole_rows() {
    // One underwriter of 10.00, one member backed to the ceiling.
    //
    // A community whose whole seed is 10.00 has a denomination to match, and
    // says so: `v_base` sets the bond unit and the establishment floor, so a
    // fixture that left it at the genesis 1,000 would be modelling members who
    // cannot afford to write in their own community.
    let mut c = Chain::founded_with(&[10.0], 3);
    c.st.params.v_base = 10.0;
    let (u, m, buyer) = (0, 1, 2);
    c.back(u, m, 10.0);
    assert_eq!(c.cap(m), 10.0);

    // Draw 8.00 of it, leaving 2.00 of headroom.
    let drawn = c.lend(u, m, 8.0);
    assert!(c.st.contracts[&drawn].insured);
    assert_eq!(c.cap(m), 2.0, "2.00 of the arc is left");

    // One row of 3.00 does not fit, so it is uninsured ENTIRELY — the
    // remaining 2.00 insures none of it.
    let mut big = Chain::founded_with(&[10.0], 3);
    big.st.params.v_base = 10.0;
    big.back(u, m, 10.0);
    big.lend(u, m, 8.0);
    let one = big.lend(buyer, m, 3.0);
    assert!(!big.st.contracts[&one].insured, "insurance is all-or-nothing per row");
    assert_eq!(big.st.committed_total(), 8.0, "so the last 2.00 of the arc goes unused");

    // The same 3.00 of trade, split into three rows, takes the 2.00.
    let mut split = Chain::founded_with(&[10.0], 3);
    split.st.params.v_base = 10.0;
    split.back(u, m, 10.0);
    split.lend(u, m, 8.0);
    let rows: Vec<ContractId> = (0..3).map(|_| split.lend(buyer, m, 1.0)).collect();
    let insured: usize = rows.iter().filter(|id| split.st.contracts[id].insured).count();
    assert_eq!(insured, 2, "two of the three fit, first-come");
    assert_eq!(split.st.committed_total(), 10.0, "and the arc is fully drawn");
}

/// **An uninsured loss does not touch the creditor's own standing, and that is
/// a decision rather than an omission.**
///
/// The other open question was whether an uninsured loss should touch the creditor's own standing. The answer is no, and the reason is the
/// free-signature bound one layer down: a debtor needs no standing to accept
/// credit, so if defaulting lowered the CREDITOR's capacity, then any key —
/// costing nothing, worth nothing — could destroy a real member's standing by
/// borrowing uninsured and walking away. The sanction would be a weapon handed
/// to the party with nothing to lose, priced at zero, and repeatable.
///
/// The second reason is that it would tax the one act that grows the graph.
/// Settlement is the only writer of a stake, so a creditor's willingness to
/// take a first uninsured risk on a stranger is precisely what confers
/// standing on newcomers; charging them for the ones that fail chills exactly
/// the trade the bootstrap depends on. And the third is evidential: capacity
/// asks whether the community will carry YOUR debt, and losing money to
/// somebody else's default is not evidence about that.
#[test]
fn an_uninsured_default_leaves_the_creditors_standing_alone() {
    // Measured against the COUNTERFACTUAL, because standing decays on its own
    // and a bare before/after would credit the default with the decay. Two
    // identical communities, the same number of epochs walked in each, and the
    // only difference is whether the uninsured claim was ever made.
    let (u, creditor, fresh) = (0, 1, 3);
    let scene = || {
        let mut c = Chain::founded(1, 3);
        c.back(u, creditor, 400.0);
        assert_eq!(c.cap(creditor), 400.0);
        c
    };

    let mut lost = scene();
    // A key that has done nothing borrows beyond any capacity it has — which
    // is zero — so the claim is uninsured and the creditor bears it alone.
    assert_eq!(lost.cap(fresh), 0.0, "it has done nothing, so it is worth nothing");
    let bad = lost.lend(creditor, fresh, 250.0);
    assert!(!lost.st.contracts[&bad].insured, "nothing backs the fresh key, so nothing is reserved");
    lost.default_on(bad);
    assert_eq!(lost.st.contracts[&bad].status, ContractStatus::Expired);

    let mut quiet = scene();
    quiet.goto(lost.st.epoch);

    assert_eq!(
        lost.cap(creditor),
        quiet.cap(creditor),
        "the creditor's standing is what OTHERS put behind them, and nobody withdrew anything"
    );
    // And the free key gained nothing by it either: the attack is refused by
    // costing nothing rather than by being detected.
    assert_eq!(lost.cap(fresh), 0.0);
}

/// **A settlement's stake is capped by what the creditor may confer once the
/// payment has released its reservation**, not by the residual that still
/// holds the obligation being paid.
///
/// One underwriter backs A for 100; A backs X and Y for 100 each; Y backs S
/// for 100. X lends S 100, insured: the reservation runs U→A→Y→S and
/// saturates U→A, which is X's only backing. S pays. Read before the release,
/// X's conferrable is the shadow of its own loan — 0.00 — and X earns nothing
/// for a loan it made and was repaid; read after, it is 100.00.
///
/// Mutation that bites: write the stake before `rehold` in `settle`, and the
/// edge X→S is absent.
#[test]
fn a_settlement_stakes_against_the_residual_the_payment_leaves() {
    let mut c = Chain::founded(1, 4);
    let (u, a, x, y, s) = (0, 1, 2, 3, 4);
    c.back(u, a, 100.0);
    c.back(a, x, 100.0);
    c.back(a, y, 100.0);
    c.back(y, s, 100.0);
    assert_eq!(c.cap(s), 100.0, "S is reachable through Y alone");
    assert_eq!(c.cap(x), 100.0, "and X through A alone");

    let loan = c.lend(x, s, 100.0);
    assert!(c.st.contracts[&loan].insured, "S has the capacity, so the loan holds a reservation");
    assert_eq!(c.cap(x), 0.0, "which took U→A, X's only backing");

    c.settle(loan, 100.0);
    assert_eq!(
        c.st.edges.get(&(x as usize, s as usize)).copied(),
        Some(State::to_minor(100.0)),
        "X conferred the full loan on S: the reservation was released before the stake was capped"
    );
    assert_eq!(c.cap(x), 100.0, "and X's own backing is back");
}

// ------------------------------------------------------- the review's probes --

mod final_review_model {
    use super::*;

    /// **Standing is the peak over an obligation's cumulative repayment.** Read
    /// per installment, a member who honoured 1,000 in two halves held a stake
    /// of 500 beside one who paid at once and held 1,000, for the same
    /// evidence; the paper's definition is stated over the obligation.
    ///
    /// Mutation that bites: pass the installment to `discharge_credit`; the
    /// two chains disagree.
    #[test]
    fn standing_is_the_peak_over_an_obligations_cumulative_repayment() {
        let mut c = Chain::founded(1, 2);
        c.back(0, 1, 2_500.0);
        let mut b = c.clone_for_control();
        let ca = c.lend(1, 2, 1_000.0);
        c.settle(ca, 1_000.0);
        let cb = b.lend(1, 2, 1_000.0);
        b.settle(cb, 500.0);
        assert_eq!(b.st.edges.get(&(1, 2)).copied(), Some(State::to_minor(500.0)), "half repaid is half the standing");
        b.settle(cb, 500.0);
        assert_eq!(c.st.edges.get(&(1, 2)), b.st.edges.get(&(1, 2)), "and the whole is the whole");
        assert_eq!((c.cap(2), b.cap(2)), (1_000.0, 1_000.0));
        // The wash bound is untouched: the same obligation again raises nothing.
        for _ in 0..3 {
            let x = c.lend(1, 2, 1_000.0);
            c.settle(x, 1_000.0);
        }
        assert_eq!(c.cap(2), 1_000.0);
    }

    /// **A partial settlement shrinks the hold where it is.** Re-solving it
    /// cost every validator a network build per payment and, on this shape,
    /// moved the hold onto an underwriter the solver preferred: D insured by
    /// U_b alone, U_a reaching D since, a payment of 100 — a re-solve answers
    /// `{U_a: 200}`, a shrink `{U_b: 200}`.
    ///
    /// Mutation that bites: release and re-reserve in `rehold` for a live row.
    #[test]
    fn a_partial_settlement_leaves_the_hold_where_it_was() {
        let mut c = Chain::founded(2, 2);
        c.back(1, 2, 300.0);
        let cid = c.lend(3, 2, 300.0);
        assert_eq!(c.st.contracts[&cid].held.supply, vec![(1, 30_000)], "insured by U_b alone");
        c.back(0, 2, 300.0);
        c.settle(cid, 100.0);
        let h = &c.st.contracts[&cid].held;
        assert_eq!(h.supply, vec![(1, 20_000)], "shrunk where it was, never re-solved onto U_a");
        assert_eq!(h.amount(), c.st.contracts[&cid].outstanding, "an insured obligation owes what it holds");
        assert_eq!(c.st.supply_floor(0), 0, "U_a is untouched");
    }
}
