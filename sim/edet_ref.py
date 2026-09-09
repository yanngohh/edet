"""A reference implementation of the v0.7.0 capacity model.

The point of this file is that it is NOT the Rust kernel written twice.
Capacity is a maximum flow, and `scipy.sparse.csgraph.maximum_flow` is an
independent implementation of maximum flow by people who had never heard of
this project — so agreement between the two says the Rust kernel computes the
right *mathematics*, not that two copies of one algorithm agree with each other.

Everything is integer, in minor units, exactly as the kernel is. Max-flow over
floating point puts branch decisions behind epsilon comparisons: two replicas
can agree on the value and still take different augmenting paths, which is a
consensus fault rather than a rounding one.
"""

from __future__ import annotations

import numpy as np
from scipy.sparse import csr_matrix
from scipy.sparse.csgraph import maximum_flow

# scipy's maximum_flow works in int32, so a sink arc cannot be "very large" —
# it has to be a real number that cannot overflow when arcs are summed during
# augmentation. The total declared supply is the exact bound: every unit of flow
# crosses exactly one supply arc, so no cut can ever exceed it, and using it
# keeps the sink arcs non-binding without inventing a magic constant.


def capacity(edges, reserved, committed, underwriters, targets, n, own_supply=False):
    """Maximum flow into `targets` from the underwriters outside them.

    `edges` and `reserved` are dicts keyed `(creditor, debtor)`; `committed` and
    `underwriters` are dicts keyed by underwriter. Underwriters INSIDE the
    measured set supply it nothing — the one clause that stops a coalition
    underwriting itself.

    `own_supply` drops that clause for a target's OWN arc. It is a CREDIT rule,
    and the write layer is the one reading that does not want it: a member's own
    declaration already counts in what they may spend, and a founding
    underwriter holds supply and no in-stakes.
    """
    targets = set(targets)
    if not targets or n == 0:
        return 0

    # Node layout: 0..n-1 accounts, n source, n+1 sink.
    src, snk = n, n + 1
    rows, cols, vals = [], [], []

    def arc(u, v, c):
        if c > 0:
            rows.append(u)
            cols.append(v)
            vals.append(int(c))

    for (c, d), w in edges.items():
        if c < n and d < n:
            arc(c, d, w - reserved.get((c, d), 0))
    for u, supply in underwriters.items():
        if u < n and (own_supply or u not in targets):
            arc(src, u, supply - committed.get(u, 0))
    ceiling = sum(underwriters.values()) or 1
    for t in targets:
        if t < n:
            arc(t, snk, ceiling)

    if not vals:
        return 0
    # scipy wants a dense-ish CSR over the node count, and duplicate (u,v)
    # pairs summed — which cannot occur here, since `edges` is keyed by the
    # pair and each underwriter contributes one source arc.
    graph = csr_matrix((vals, (rows, cols)), shape=(n + 2, n + 2), dtype=np.int32)
    return int(maximum_flow(graph, src, snk).flow_value)


def conferrable(edges, reserved, committed, underwriters, member, n):
    """A member's declared supply if they underwrite, else their own capacity.

    The two are different quantities and the model turns on not conflating
    them: capacity is how much the community will carry YOU, supply is how much
    you have promised to carry others.
    """
    if member in underwriters:
        return underwriters[member]
    return capacity(edges, reserved, committed, underwriters, [member], n)


def stake(edges, creditor, debtor, amount, may_confer):
    """The §Standing settlement rule: a peak, capped by what the creditor may confer."""
    if creditor == debtor:
        return
    placed = min(amount, may_confer)
    if placed > 0:
        edges[(creditor, debtor)] = max(edges.get((creditor, debtor), 0), placed)


def decay(edges, reserved, num, den):
    """Decay every edge by num/den, never below its live reservation."""
    if den == 0 or num >= den:
        return
    for k in list(edges):
        floor = reserved.get(k, 0)
        edges[k] = max((edges[k] * num) // den, floor)
        if edges[k] == 0:
            del edges[k]


def reserve(edges, reserved, committed, underwriters, debtor, n, amount, own_supply=False):
    """§Standing: take exactly `amount` of flow into `debtor` and return what it took.

    Returns `None` and changes nothing if the residual cannot carry it;
    otherwise mutates `reserved` and `committed` and returns the augmentation
    as ``{"edges": {(c, d): units}, "supply": {u: units}}``.

    **Both halves of the path are charged.** The supply arc is the one every
    debtor of an underwriter shares, and leaving it uncharged is the difference
    between a bound and a suggestion — measured before the kernel corrected it,
    one supply of 2500 carried 250,000 across a hundred debtors.

    The amount is imposed by capping the SINK arc rather than by asking for a
    maximum and truncating it: scipy computes a maximum flow, and a maximum
    truncated after the fact is not an augmentation of the size requested.

    **Ledger edges are split through a node of their own**, which is not
    decoration. scipy reports its answer as an antisymmetric flow matrix, so
    two accounts that back each OTHER — an ordinary pair, and the shape the
    wash probe builds deliberately — are reported netted: 5 one way and 0 the
    other is indistinguishable from 6 and 1. A decomposition is what a
    reservation records, so it has to be unambiguous, and a dedicated node per
    DIRECTED pair is what makes it so without a second flow implementation.
    """
    if amount <= 0:
        return {"edges": {}, "supply": {}}

    src, snk = n, n + 1
    mid = {}
    for key in sorted(edges):
        c, d = key
        if c < n and d < n and edges[key] - reserved.get(key, 0) > 0:
            mid[key] = n + 2 + len(mid)
    size = n + 2 + len(mid)

    rows, cols, vals = [], [], []

    def arc(u, v, c):
        if c > 0:
            rows.append(u)
            cols.append(v)
            vals.append(int(c))

    for key, m in mid.items():
        c, d = key
        free = edges[key] - reserved.get(key, 0)
        arc(c, m, free)
        arc(m, d, free)
    supply_node = {}
    for u, supply in sorted(underwriters.items()):
        if u < n and (own_supply or u != debtor):
            free = supply - committed.get(u, 0)
            if free > 0:
                supply_node[u] = free
                arc(src, u, free)
    arc(debtor, snk, amount)

    if not vals:
        return None
    graph = csr_matrix((vals, (rows, cols)), shape=(size, size), dtype=np.int32)
    res = maximum_flow(graph, src, snk)
    if int(res.flow_value) < amount:
        return None

    flow = res.flow.tocsr()
    held_edges = {}
    for key, m in mid.items():
        units = int(flow[key[0], m])
        if units > 0:
            held_edges[key] = units
    held_supply = {}
    for u in supply_node:
        units = int(flow[src, u])
        if units > 0:
            held_supply[u] = units

    for key, units in held_edges.items():
        reserved[key] = reserved.get(key, 0) + units
    for u, units in held_supply.items():
        committed[u] = committed.get(u, 0) + units
    return {"edges": held_edges, "supply": held_supply}


def release(reserved, committed, held):
    """§Standing: settlement gives back precisely what acceptance took.

    Not a proportional credit across the arcs incident to the debtor — that is
    the inverse of nothing, and it strands the upstream half of every multi-hop
    path, so settling an obligation destroys capacity nothing was behind.
    """
    for key, units in held["edges"].items():
        left = reserved.get(key, 0) - units
        if left > 0:
            reserved[key] = left
        else:
            reserved.pop(key, None)
    for u, units in held["supply"].items():
        left = committed.get(u, 0) - units
        if left > 0:
            committed[u] = left
        else:
            committed.pop(u, None)


def held_amount(held):
    """What a reservation holds, read off the supply side.

    Every insured unit crosses exactly one supply arc, so the supply side IS
    the total — which is also why §Verification's second invariant can compare it against
    the book exactly rather than approximately.
    """
    return sum(held["supply"].values())
