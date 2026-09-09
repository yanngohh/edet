"""The §Security claims, probed against the reference implementation.

These are not the kernel's own tests restated. They are the claims the design
MAKES, checked against an independent max-flow — so a claim that survives here
survived an implementation that shares no code with the one it is about.

Every probe asserts over a SET where the claim is about a set. Every defect this
model has carried was a bound asserted over a set and tested over a singleton,
and a per-account check passes in every one of them.
"""

from __future__ import annotations

import edet_ref as ref

SUPPLY = 250_000
HONEST = 30_000  # 300.00, one ordinary trade


def founded(k):
    return {i: SUPPLY for i in range(k)}


def back(edges, uw, creditor, debtor, n):
    """Creditor backs debtor for everything they may confer."""
    ref.stake(edges, creditor, debtor, 1 << 40, ref.conferrable(edges, {}, {}, uw, creditor, n))


def cut(edges, uw, targets, n):
    return ref.capacity(edges, {}, {}, uw, targets, n)


def minting_identities_confers_nothing():
    uw, e, n = founded(6), {}, 200
    sybils = list(range(100, 160))
    return cut(e, uw, sybils, n) == 0, "60 fresh keys, measured together"


def wash_trading_confers_nothing():
    uw, e, n = founded(6), {}, 60
    for _ in range(1000):
        back(e, uw, 40, 41, n)
        back(e, uw, 41, 40, n)
    return cut(e, uw, [40, 41], n) == 0, "2000 cycles between two accounts nobody backs"


def selling_confers_nothing():
    """Flow runs creditor to debtor, so standing is evidence of having OWED."""
    uw, e, n = founded(6), {}, 120
    buyers = list(range(10, 30))
    for i, b in enumerate(buyers):
        back(e, uw, i % 6, b, n)
    seller = 90
    for _ in range(100):
        for b in buyers:
            back(e, uw, seller, b, n)
    before = cut(e, uw, [seller], n)
    back(e, uw, buyers[0], seller, n)  # one honoured purchase
    return before == 0 and cut(e, uw, [seller], n) == SUPPLY, "2000 sales then one purchase"


def one_supply_is_lent_once():
    ok = True
    for k in (2, 5, 20, 100):
        uw, e, n = {0: SUPPLY}, {}, k + 2
        for d in range(1, k + 1):
            back(e, uw, 0, d, n)
        singles = all(cut(e, uw, [d], n) == SUPPLY for d in range(1, k + 1))
        together = cut(e, uw, list(range(1, k + 1)), n) == SUPPLY
        ok = ok and singles and together
    return ok, "k = 2, 5, 20, 100: each looks fully backed, together they are one supply"


def an_underwriter_is_bounded_by_what_they_declared():
    uw, e, n = founded(6), {}, 80
    sybils = list(range(50, 70))
    for _ in range(50):
        for s in sybils:
            back(e, uw, 0, s, n)
            back(e, uw, s, 0, n)  # back-staking
    return cut(e, uw, sybils, n) == SUPPLY, "20 sybils, 50 rounds, with back-staking"


def a_coalition_cannot_underwrite_itself():
    """True, and it measures the DECLARERS. See the probe below for who the
    credit actually went to."""
    uw, e, n = dict(founded(6)), {}, 80
    back(e, uw, 0, 50, n)
    external = cut(e, uw, [50], n)
    coalition = [50] + list(range(60, 75))
    ok = True
    for _ in range(8):
        for m in coalition:
            c = cut(e, uw, [m], n)
            if c > 0 and m not in uw:
                uw[m] = c
        for m in coalition:
            for s in coalition:
                back(e, uw, m, s, n)
        outside = {k: v for k, v in uw.items() if k not in coalition}
        ok = ok and cut(e, outside, coalition, n) == external
    return ok, "8 rounds of self-appointment, pinned at external backing"


def a_coalition_cannot_insure_the_accounts_it_backs():
    """Underwriting the accounts a coalition BACKS, measured against the reference.

    The probe above is true and answers about the wrong party. A coalition
    cannot underwrite ITSELF — a cut over a set draws only on underwriters
    outside it — and a rule that let it declare against conferred capacity
    would let it underwrite the accounts it BACKS, which is where the credit
    goes. Twelve joiners wash-backing each other behind a seed of 100, each
    declaring the maximum its own capacity admits, then one fresh sybil apiece:
    the sybils' cut runs to 2,048x the seed, every unit of it insured by the
    ledger's own label, with every invariant satisfied.

    Measured both ways, because a rule has to be measured against the
    counterfactual: `declared` is the declarable rule, `seated` is the one this
    ledger runs (a supply is a source only where a ceremony put it). The claim
    is the second column, and the first is what says the scene is the one that
    matters.
    """
    seed = 10_000  # 100.00
    joiners = list(range(20, 32))
    sybils = list(range(40, 52))
    n = 200

    def build(internal_declarations):
        uw, e = {0: seed}, {}
        back(e, uw, 0, joiners[0], n)
        for i, j in enumerate(joiners):
            for prev in joiners[:i]:
                back(e, uw, prev, j, n)
            if internal_declarations:
                c = cut(e, uw, [j], n)
                if c > 0:
                    uw[j] = c
        for j, sy in zip(joiners, sybils):
            back(e, uw, j, sy, n)
        return uw, e

    uw_d, e_d = build(True)
    uw_s, e_s = build(False)
    declared = cut(e_d, uw_d, sybils, n)
    seated = cut(e_s, uw_s, sybils, n)
    return (
        declared > 100 * seed and seated <= seed,
        f"12 joiners, 12 sybils on a seed of {seed}: declarable rule insures {declared} "
        f"({declared // seed}x), ceremony-seated rule insures {seated}",
    )


def usage_cannot_seed_insured_credit():
    """Zero is absorbing: with no underwriter, trade creates nothing, for ever."""
    uw, e, n = {}, {}, 60
    for _ in range(500):
        back(e, uw, 10, 11, n)
        back(e, uw, 11, 12, n)
    return len(e) == 0 and cut(e, uw, list(range(10, 13)), n) == 0, "1000 trades, no underwriters"


def a_pool_funded_from_inside_relabels_the_seed():
    """A mutual pool funded by a levy on settlements adds nothing to the seed.

    §Recourse's load-bearing measurement, and the paper had been citing it without
    a probe behind it. A levy is a share of a settlement, and a settlement is a
    signature, so the pool is funded by exactly the quantity
    `ass:free-signatures` says cannot seed insured credit. Two halves:

    - **Founderless.** Nothing may be conferred, so nothing is staked, so the
      levy is a percentage of nothing — for ever, however much trade happens.
    - **Seeded.** Measure the community WITH the pool in the target set: the
      cut is still exactly the seed. A pool funded from inside is a
      re-labelling of the seed rather than an addition to it, which is why
      retiring the ledger's loss pool cost the model nothing it had.
    """
    POOL = 900
    LEVY_NUM, LEVY_DEN = 1, 20

    def levy(edges, uw, creditor, debtor, n):
        """Settle creditor->debtor, then stake the pool's share of it."""
        back(edges, uw, creditor, debtor, n)
        share = edges.get((creditor, debtor), 0) * LEVY_NUM // LEVY_DEN
        ref.stake(edges, creditor, POOL, share, ref.conferrable(edges, {}, {}, uw, creditor, n))

    # Founderless: 10,000 settlements, and the pool is worth zero throughout.
    uw, e, n = {}, {}, 1000
    for i in range(10_000):
        levy(e, uw, 10 + (i % 5), 20 + (i % 7), n)
    founderless = cut(e, uw, [POOL], n) == 0 and len(e) == 0

    # Seeded: the pool accumulates real stake, and the community measured
    # TOGETHER WITH it still owes exactly the seed.
    uw, e, n = founded(6), {}, 1000
    members = list(range(10, 30))
    for i, m in enumerate(members):
        back(e, uw, i % 6, m, n)
    for i in range(10_000):
        levy(e, uw, members[i % len(members)], members[(i + 1) % len(members)], n)
    funded = cut(e, uw, [POOL], n) > 0
    seed = 6 * SUPPLY
    together = cut(e, uw, members + [POOL], n) == seed

    return (
        founderless and funded and together,
        f"10,000 levied settlements: founderless pool 0; seeded, members+pool = the seed {seed}",
    )


def one_ledger_hosts_many_communities():
    """The boundary between two communities is the absence of a path."""
    uw, e, n = {0: SUPPLY, 1: SUPPLY}, {}, 40
    bakers, library = [10, 11], [20, 21]
    for m in bakers:
        back(e, uw, 0, m, n)
    for m in library:
        back(e, uw, 1, m, n)
    apart = cut(e, uw, bakers, n) == SUPPLY and cut(e, uw, library, n) == SUPPLY
    together = cut(e, uw, bakers + library, n) == 2 * SUPPLY
    back(e, uw, bakers[0], library[0], n)  # one trade across
    joined = cut(e, uw, library, n) > SUPPLY
    return apart and together and joined, "two co-ops on one ledger, separate until they trade"


def decay_cannot_release_live_collateral():
    uw, e, n = founded(6), {}, 60
    back(e, uw, 0, 10, n)
    reserved = {(0, 10): 200_000}
    for _ in range(24):
        ref.decay(e, reserved, 977, 1000)
    return e[(0, 10)] >= 200_000, "24 epochs at 80% reserved"


def a_colluder_passes_on_exactly_what_they_hold():
    """`cor:collusion`: a non-underwriting colluder is a vertex separator.

    A claim's wording matching is not the same as its measurement existing, so
    this runs it rather than restating it.

    One honest 300 trade backs the colluder, who then puts everything they may
    confer behind forty accounts, which pump each other for eight rounds at
    whatever each may confer. Every path into the ring passes through the
    colluder, so the ring — measured as a SET, which is the quantifier the
    corollary uses — holds exactly what the colluder holds, and holds it
    however many accounts are in the ring and however large the stakes between
    them.
    """
    uw, e, n = founded(6), {}, 200
    colluder, ring = 50, list(range(60, 100))
    ref.stake(e, 0, colluder, HONEST, ref.conferrable(e, {}, {}, uw, 0, n))
    held = cut(e, uw, [colluder], n)
    worst = 0
    for _ in range(8):
        confer = ref.conferrable(e, {}, {}, uw, colluder, n)
        for a in ring:
            ref.stake(e, colluder, a, 1 << 40, confer)
        for a in ring:
            confer = ref.conferrable(e, {}, {}, uw, a, n)
            for b in ring:
                ref.stake(e, a, b, 1 << 40, confer)
        worst = max(worst, cut(e, uw, ring, n), cut(e, uw, ring + [colluder], n))
    return (
        held == HONEST and worst == HONEST,
        f"own capacity {HONEST}, 40 accounts pumping 8 rounds: region capacity {worst}",
    )


def attacking_costs_what_participating_costs():
    """`cor:linear`: capacity extracted is linear in real backing, at 1.00x.

    The other claim the paper measured against a formulation that no longer
    exists. An adversary buys genuine relationships one at a time and spreads
    the proceeds over five keys; what the five hold TOGETHER is what was put
    behind them, at every k from 1 to 20. Splitting across identities is what
    the ratio is testing — a per-account reading would pass in a scene where
    the set overshot.
    """
    uw, e, n = founded(6), {}, 200
    honest, keys = list(range(10, 30)), list(range(100, 105))
    for i, h in enumerate(honest):
        back(e, uw, i % 6, h, n)
    ratios = set()
    for k in range(1, 21):
        h = honest[k - 1]
        ref.stake(e, h, keys[(k - 1) % len(keys)], HONEST, ref.conferrable(e, {}, {}, uw, h, n))
        ratios.add(cut(e, uw, keys, n) / (k * HONEST))
    return ratios == {1.0}, "1-20 genuine relationships across five keys: ratio 1.00"


def a_write_floor_is_not_spent_by_somebody_elses_credit():
    """`prop:write-floor` is GROSS of live credit.

    The floor is the max-flow over the arcs a ceremony seated, and a member's
    own debt and encumbrance come off it afterwards. Read on the RESIDUAL graph
    instead — which the implementation did, mirroring `conferrable`, where
    netting is the whole point — it also came off for everybody ELSE's debt, and
    a member's own came off twice.

    Measured here on the mathematics rather than on the ledger: two members
    backed identically by one small underwriter, a second larger underwriter
    that reaches neither, and one of the two draws its backer dry. Community
    utilisation **20%**; the other member has borrowed nothing.
    """
    uw, e, n = {0: HONEST, 1: 4 * HONEST}, {}, 60
    external = dict(uw)
    quiet, borrower = 10, 11
    for m in (quiet, borrower):
        ref.stake(e, 0, m, HONEST, ref.conferrable(e, {}, {}, uw, 0, n))

    floor_before = ref.capacity(e, {}, {}, external, [quiet], n)
    held = ref.reserve(e, {}, {}, uw, borrower, n, HONEST)
    reserved = {k: v for k, v in held["edges"].items()}
    committed = dict(held["supply"])
    assert ref.held_amount(held) == HONEST

    credit_after = ref.capacity(e, reserved, committed, uw, [quiet], n)
    floor_after = ref.capacity(e, {}, {}, external, [quiet], n)
    floor_residual = ref.capacity(e, reserved, committed, external, [quiet], n)
    utilisation = sum(committed.values()) / sum(uw.values())
    return (
        floor_before == floor_after == HONEST and credit_after == 0 and floor_residual == 0,
        f"utilisation {utilisation:.0%}: their credit {credit_after}, their floor {floor_after} "
        f"(residual reading gave {floor_residual})",
    )


def the_write_floor_gives_a_coalition_what_it_gives_honest_members():
    """§H's 1.00x parity, re-measured under the gross reading.

    §H moved the write floor off the declared supply because a chain of internal
    declarations doubled it per accomplice — 614,400 at twelve links against a
    real cut of 300. Taking the floor gross of live credit changes the netting
    and must not change that: the arcs it runs over are still the seeded ones,
    so a declaration nobody seeded is still worth nothing to it.

    **The ledger refuses those declarations outright**, so this is the second
    line rather than the first, and it is kept for exactly that reason: it
    models the door being open and shows the write floor holding anyway. `uw`
    here is a rule the transition function does not implement, and `external`
    is the one it does.
    """
    ratios = set()
    for k in (3, 7, 12):
        uw, e, n = founded(6), {}, 200
        external = dict(uw)
        # The register's own construction: one honest round-trip seeds the head,
        # every accomplice after it is fake-backed by ALL the previous ones —
        # the fan-in is where the doubling came from, not the chain — and then
        # declares the maximum §Standing allows.
        chain = list(range(20, 20 + k))
        ref.stake(e, 0, chain[0], HONEST, ref.conferrable(e, {}, {}, uw, 0, n))
        uw[chain[0]] = ref.capacity(e, {}, {}, uw, [chain[0]], n)
        for a in chain[1:]:
            for prev in chain[: chain.index(a)]:
                ref.stake(e, prev, a, HONEST * 4096, ref.conferrable(e, {}, {}, uw, prev, n))
            uw[a] = ref.capacity(e, {}, {}, uw, [a], n)
        declared = sum(uw[a] for a in chain)
        coalition = sum(ref.capacity(e, {}, {}, external, [a], n) for a in chain)

        uw2, e2 = founded(6), {}
        honest = list(range(40, 40 + k))
        for h in honest:
            ref.stake(e2, 0, h, HONEST, ref.conferrable(e2, {}, {}, uw2, 0, n))
        theirs = sum(ref.capacity(e2, {}, {}, dict(uw2), [h], n) for h in honest)
        ratios.add(coalition / theirs)
        # Non-vacuity: the scene has to still be the one §H measured. What the
        # chain DECLARES runs away — 1.33x the floor at three links, 9x at
        # seven, 171x at twelve, with the last accomplice alone declaring
        # 307,200 — while what the seed reaches stays k trades of 300.
        assert declared > coalition, f"the chain must still inflate what it declares at k={k}"
        assert coalition == k * HONEST, f"and the floor must stay one honest trade each at k={k}"
        inflation = declared / coalition
    return ratios == {1.0}, f"k = 3, 7, 12 against k honest members: ratio 1.00 (declared runs to {inflation:.0f}x)"


def the_bound_holds_under_mixed_traffic():
    """§Standing on the RESERVE path, driven until the ceiling binds.

    Everything above is a `capacity()` query, and this design has already paid
    once for a table measured on one path only: reserving against twenty-one
    debtors of one underwriter yielded 52,500 against a cut of 2,500, because
    the supply arc every one of them shares was not charged. So this drives the
    reservation path itself — four thousand deterministic reserve and release
    steps over a two-hop graph — and audits invariant 1 at BOTH quantifiers
    after every one: the aggregate set, and (every eighth step) each family of
    debtors drawing through one underwriter, which is the family a per-account
    check is blind to.

    Two claims come out of one run. The bound holds under traffic that presses
    it — a third of the reservations are refused, so it binds rather than
    idles — and settlement gives back the whole path, since releasing every
    live hold at the end drains reserved and committed to exactly zero. A
    proportional release would strand the upstream hop of every multi-hop hold
    and leave a residue here.
    """
    uw, e, n = founded(6), {}, 120
    first, second = list(range(10, 30)), list(range(40, 50))
    for i, b in enumerate(first):
        back(e, uw, i % 6, b, n)
    for j, s in enumerate(second):
        back(e, uw, first[j % len(first)], s, n)
    debtors = first + second
    ceiling = cut(e, uw, debtors, n)

    reserved, committed, live = {}, {}, []
    seed = [12345]

    def roll(mod):
        """A deterministic schedule: a probe that differs per run is not a gate."""
        seed[0] = (seed[0] * 1103515245 + 12345) % (1 << 31)
        return seed[0] % mod

    checks = violations = peak = refused = 0
    for step in range(4000):
        if live and roll(3) == 0:
            _, held = live.pop(roll(len(live)))
            ref.release(reserved, committed, held)
        else:
            d = debtors[roll(len(debtors))]
            held = ref.reserve(e, reserved, committed, uw, d, n, (roll(30) + 1) * 10_000)
            if held is None:
                refused += 1
            else:
                live.append((d, held))
        drawn = sum(committed.values())
        peak = max(peak, drawn)
        checks += 1
        violations += drawn > ceiling
        if step % 8 == 0:
            for u in uw:
                through = sorted({d for d, h in live if u in h["supply"]})
                if not through:
                    continue
                checks += 1
                total = sum(ref.held_amount(h) for d, h in live if d in through)
                violations += total > cut(e, uw, through, n)

    for _, held in live:
        ref.release(reserved, committed, held)
    drained = not reserved and not committed
    return (
        violations == 0 and drained and refused > 0 and peak > ceiling * 3 // 4,
        f"4,000 steps, {checks} set checks, {violations} violations; peak {peak} against a ceiling of {ceiling}",
    )


def seats_are_bounded_by_the_cut():
    """`cor:seats`: a row is a stock, and its price is a reservation on the graph.

    Each seat takes one bond unit of flow from the seed to its SPONSOR, on a
    reservation map of its own, and nothing releases it. So the rows a set can
    seat are bounded by the peak of the arcs reaching that set from outside,
    over the bond unit — and the whole point of measuring it here is that the
    reference shares no code with the kernel that enforces it.

    The farm shape: one underwriter, one operator behind a real 500.00 edge,
    children behind wash edges of 100.00. The operator seats until refused, and
    the count is checked at every prefix as well as at the end, because a bound
    that holds only in aggregate is a bound tested at the wrong quantifier.
    """
    unit = 2_000  # BOND_UNIT_FRACTION x v_base = 0.02 x 1,000, in minor units
    n = 400
    uw = {0: SUPPLY}
    edges = {(0, 1): 50_000}  # one honest edge of 500.00
    seat_reserved, seat_committed = {}, {}

    seats = 0
    children = []
    for i in range(2, 60):
        held = ref.reserve(edges, seat_reserved, seat_committed, uw, 1, n, unit, own_supply=True)
        if held is None:
            break
        seats += 1
        children.append(i)
        # The wash the farm pays each child, which is what would make the child
        # a seater under a per-account price. It is internal to the set, so it
        # crosses no boundary and buys nothing.
        edges[(1, i)] = 10_000
        # And the prefix has to hold too, not only the final count.
        boundary = sum(w for (c, d), w in edges.items() if d in children + [1] and c not in children + [1])
        boundary += uw.get(1, 0)
        if unit * seats > boundary:
            return False, f"{seats} seats past a boundary of {boundary} at prefix {i}"

    boundary = sum(w for (c, d), w in edges.items() if d in children + [1] and c not in children + [1])
    exact = boundary // unit
    return (
        seats == exact,
        f"one edge of 500.00 carries {seats} rows against a cut of {boundary} over a unit of {unit}",
    )


PROBES = [
    minting_identities_confers_nothing,
    wash_trading_confers_nothing,
    selling_confers_nothing,
    one_supply_is_lent_once,
    an_underwriter_is_bounded_by_what_they_declared,
    a_coalition_cannot_underwrite_itself,
    a_coalition_cannot_insure_the_accounts_it_backs,
    usage_cannot_seed_insured_credit,
    a_pool_funded_from_inside_relabels_the_seed,
    one_ledger_hosts_many_communities,
    decay_cannot_release_live_collateral,
    a_colluder_passes_on_exactly_what_they_hold,
    attacking_costs_what_participating_costs,
    the_bound_holds_under_mixed_traffic,
    a_write_floor_is_not_spent_by_somebody_elses_credit,
    the_write_floor_gives_a_coalition_what_it_gives_honest_members,
    seats_are_bounded_by_the_cut,
]


def run():
    failures = 0
    for probe in PROBES:
        ok, detail = probe()
        print(f"  {'PASS' if ok else 'FAIL'}  {probe.__name__:48s} {detail}")
        failures += not ok
    return failures
