#!/usr/bin/env python3
"""Regenerate `fixtures/kernel.json` from the reference implementation.

Each case is a whole scene — a stake graph, an underwriter set, a measured
account set — and the expected answer scipy's maximum flow gives for it.
`crates/kernel/tests/fixtures.rs` replays every one against the Rust kernel and
demands BIT-IDENTICAL agreement, which is a stronger demand than a
fixtures made and is available here only because the whole capacity path is
integer. A tolerance would be hiding something: two exact algorithms over the
same integers have nothing to disagree about.
"""

from __future__ import annotations

import json
import pathlib

import edet_ref as ref

SUPPLY = 250_000  # 2500.00 in minor units


def founded(k):
    return {i: SUPPLY for i in range(k)}


def case(name, why, edges, underwriters, targets, n, reserved=None, committed=None, own_supply=False):
    reserved = reserved or {}
    committed = committed or {}
    return {
        "name": name,
        "why": why,
        "n": n,
        "edges": [[list(k), v] for k, v in sorted(edges.items())],
        "reserved": [[list(k), v] for k, v in sorted(reserved.items())],
        "committed": [[k, v] for k, v in sorted(committed.items())],
        "underwriters": [[k, v] for k, v in sorted(underwriters.items())],
        "targets": sorted(targets),
        "own_supply": own_supply,
        "capacity": ref.capacity(edges, reserved, committed, underwriters, targets, n, own_supply),
    }


def build():
    cases = []

    # A key nobody has backed is worth nothing, by arithmetic.
    cases.append(case("fresh key", "no incident stakes", {}, founded(6), [42], 60))

    # One backed newcomer, and the chain that carries their standing onward
    # undiminished — while the chain AS A SET can only owe one supply.
    chain = {(0, 10): SUPPLY, (10, 11): SUPPLY, (11, 12): SUPPLY}
    cases.append(case("backed newcomer", "one hop from an underwriter", chain, founded(6), [10], 60))
    cases.append(case("three hops out", "standing propagates undiminished", chain, founded(6), [12], 60))
    cases.append(case("the chain as a set", "simultaneous use is one supply", chain, founded(6), [10, 11, 12], 60))

    # Wash trading between two accounts nobody backs.
    wash = {(40, 41): SUPPLY, (41, 40): SUPPLY}
    cases.append(case("wash pair", "internal edges never cross the set's own boundary", wash, founded(6), [40, 41], 60))

    # One underwriter, many debtors: the scene that hid the worst defect this
    # model has carried. Each debtor looks fully backed; together they are one
    # supply, and only the SET says so.
    for k in (2, 5, 20):
        many = {(0, d): SUPPLY for d in range(1, k + 1)}
        cases.append(
            case(f"one supply, {k} debtors", "the supply arc is shared", many, {0: SUPPLY}, list(range(1, k + 1)), k + 2)
        )
        cases.append(case(f"one supply, {k} debtors, singleton", "a per-account check passes here", many, {0: SUPPLY}, [1], k + 2))

    # A coalition cannot underwrite itself: appointing your own accomplices adds
    # nothing, because the measurement excludes them along with you.
    coalition = {(0, 50): SUPPLY, (50, 60): SUPPLY, (60, 50): SUPPLY}
    uw = {**founded(6), 50: SUPPLY, 60: SUPPLY}
    cases.append(case("self-underwriting coalition", "supply comes only from outside the set", coalition, uw, [50, 60], 80))

    # Reservation: the residual is what a later query sees, on both halves of
    # the path — the stake edges and the supply arc every debtor shares.
    backed = {(0, 10): SUPPLY}
    cases.append(
        case(
            "partially drawn",
            "free capacity is the stake net of what is held",
            backed,
            founded(6),
            [10],
            60,
            reserved={(0, 10): 200_000},
            committed={0: 200_000},
        )
    )
    cases.append(
        case(
            "fully drawn",
            "and nothing is left when it is all held",
            backed,
            founded(6),
            [10],
            60,
            reserved={(0, 10): SUPPLY},
            committed={0: SUPPLY},
        )
    )

    # An underwriter withdrawn to the committed floor still carries what stands
    # on them; below it, the bound breaks — which is why §Stability floors it.
    cases.append(case("supply at its floor", "a legal withdrawal holds the bound", backed, {0: 200_000}, [10], 60,
                      reserved={(0, 10): 200_000}, committed={0: 200_000}))

    # The write layer's reading, where a member's own declaration counts. The
    # exclusion above is a CREDIT rule; measured both ways on the same scene,
    # the difference IS the target's free supply.
    for own in (False, True):
        cases.append(case("a founder with no in-stakes", "supply and nothing backing them", {}, founded(2), [0], 20, own_supply=own))
        cases.append(
            case(
                "a founder already drawn on",
                "the residual of their own arc",
                {},
                founded(2),
                [0],
                20,
                committed={0: 200_000},
                own_supply=own,
            )
        )
        cases.append(
            case(
                "a founder who is also backed",
                "their own arc adds to what reaches them",
                {(1, 0): SUPPLY},
                founded(2),
                [0],
                20,
                own_supply=own,
            )
        )
        cases.append(
            case(
                "a self-underwriting coalition, write layer",
                "own supply is one member's, not the set's",
                coalition,
                uw,
                [50, 60],
                80,
                own_supply=own,
            )
        )

    return {"version": "0.7.0", "units": "minor", "cases": cases}


if __name__ == "__main__":
    out = pathlib.Path(__file__).parent / "fixtures" / "kernel.json"
    out.parent.mkdir(exist_ok=True)
    out.write_text(json.dumps(build(), indent=2) + "\n")
    print(f"wrote {out} ({len(build()['cases'])} cases)")
