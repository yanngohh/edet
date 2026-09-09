#!/usr/bin/env python3
r"""Gate the client's read-view types against what the node actually serves.

`ui/src/lib/api.ts` declares an interface per read view, and nothing read both
sides. The drift is not hypothetical and it is not cosmetic:

  * `/proposals` served `active_members`, and the wallet rendered
    ``ceil(theta_adopt * active_members)`` as "N assents needed" -- a headcount
    quorum, which is a rule this ledger has never run at any point.
  * Six admission fields were declared in the client and served by no node.
    One of them, `open_admission`, gated the wallet's only working onboarding
    card: it read `undefined`, so the card was never rendered and the client
    could not onboard anybody.
  * `NetworkView` declaring `phi`, `gauge_g` and `kappa_vol` -- an activity
    gauge, macroprudential governor and volatility term -- and the network page
    RENDERED two of them, as "Activity phi" and a "Community brake" showing
    `NaN%` under help text telling every member that "new credit is being
    tightened across the community to contain contagion risk". There is no such
    brake in this design and its absence is deliberate.

**A client type is not a claim about what the node serves**, and a view that
describes a mechanism the chain does not run is read as a promise.

Direction. This fails on a field the CLIENT declares and the node does not
serve. The converse is deliberately allowed: the node serving something no
client reads yet is a surface, not a defect.

Two modes, matching `paper-constants.py` and `proof-fixture.py`:
  * default: report (`just view-shape`).
  * `--check`: exit 1 on drift (`just view-shape-check`, in `ci`).

Stdlib-only, so a gate that runs on every push cannot break on a dependency of
its own.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
API_TS = REPO_ROOT / "ui/src/lib/api.ts"

# Fields a client legitimately declares that no node serves, with the reason.
# Keep this list SHORT and each entry justified: it is the escape hatch that
# would let the whole gate rot if it grew.
CLIENT_ONLY: dict[str, dict[str, str]] = {
    # `/whois` answers a bare `{"member": id|null}`; the client widens it with
    # what it resolved locally so a caller has one object to read.
    "WhoisResult": {
        "query": "the needle the client asked with, echoed back locally",
        "address": "derived client-side from the resolved member's key",
    },
}


def emitted() -> dict[str, dict[str, str]]:
    """Field name -> served JSON type per view, from the normative implementation."""
    out = subprocess.run(
        ["cargo", "run", "--quiet", "-p", "edet-node", "--features", "serve", "--example", "view_shape"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
    )
    if out.returncode != 0:
        sys.stderr.write(out.stderr)
        raise SystemExit(f"view-shape: the example failed (exit {out.returncode})")
    return json.loads(out.stdout)


# TypeScript declarations, reduced to the JSON type they describe. Anything
# not in this table is left as None and skipped: the gate refuses to guess.
def alias_kind(source: str, name: str) -> str | None:
    """The JSON type an `export type X = ...` alias describes, or None.

    Needed because a status is declared as a union of string literals and reads
    as a capitalised identifier, which is otherwise indistinguishable from an
    interface.
    """
    match = re.search(rf"^export type {name}\s*=([^;]+);", source, re.M)
    if not match:
        return None
    parts = [p.strip() for p in match.group(1).split("|") if p.strip()]
    if parts and all(re.fullmatch(r"'[^']*'|\"[^\"]*\"", p) for p in parts):
        return "string"
    if parts and all(re.fullmatch(r"-?\d+(\.\d+)?", p) for p in parts):
        return "number"
    return None


def ts_kind(decl: str, source: str = "") -> str | None:
    """The JSON type a TypeScript field declaration describes, or None."""
    d = decl.strip().rstrip(";").strip()
    # A nullable declaration is compared on its non-null half; `null` on the
    # wire is what the served side reports as "null" and both are skipped.
    parts = [p.strip() for p in d.split("|") if p.strip() not in ("null", "undefined")]
    if len(parts) != 1:
        return None
    one = parts[0]
    if one.endswith("[]") or one.startswith("Array<"):
        return "array"
    if one in ("number", "string", "boolean"):
        return one
    if one.startswith("{"):
        return "object"
    if one[:1].isupper() and one.isidentifier():
        return alias_kind(source, one) or "object"
    return None


def declared(source: str, name: str) -> dict[str, str | None] | None:
    """Field name -> JSON type per exported interface, or None if not there.

    Parsed rather than type-checked on purpose: the whole point is a gate that
    reads BOTH sides, and TypeScript erases these at compile time, so there is
    no runtime value for a vitest to compare. A brace-counting scan is enough
    because these interfaces are flat records of scalars, arrays and named
    types -- an inline object literal would nest, and `depth` below is what
    keeps its keys out.
    """
    match = re.search(rf"^export interface {name}\b[^{{]*{{", source, re.M)
    if not match:
        return None
    fields: dict[str, str | None] = {}
    depth = 0
    for line in source[match.end() :].splitlines():
        stripped = line.strip()
        if depth == 0 and stripped.startswith("}"):
            break
        if depth == 0:
            field = re.match(r"(\w+)\??\s*:(.*)", stripped)
            if field:
                fields[field.group(1)] = ts_kind(field.group(2), source)
        depth += line.count("{") - line.count("}")
    return fields


def extends(source: str, name: str) -> tuple[str, set[str]] | None:
    """(parent interface, fields the child deliberately re-declares).

    `Omit<Parent, 'f' | 'g'>` is how a child says the two views serve genuinely
    different things under one name --- `supply` is a scalar on a members-list
    row and the three-part underwriter object on a member detail --- so the
    omitted names are checked against the CHILD's own declaration and never
    inherited from the parent.
    """
    match = re.search(rf"^export interface {name}\s+extends\s+([^{{]+){{", source, re.M)
    if not match:
        return None
    clause = match.group(1).strip()
    omit = re.fullmatch(r"Omit<\s*(\w+)\s*,(.+)>", clause)
    if omit:
        names = {a or b for a, b in re.findall(r"'([^']*)'|\"([^\"]*)\"", omit.group(2))}
        return omit.group(1), {n for n in names if n}
    plain = re.fullmatch(r"\w+", clause)
    return (clause, set()) if plain else None


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="exit 1 on drift")
    args = parser.parse_args(argv)

    served = emitted()
    source = API_TS.read_text(encoding="utf-8")

    children: dict[str, list[str]] = {}
    for name in served:
        rel = extends(source, name)
        if rel:
            children.setdefault(rel[0], []).append(name)

    problems: list[str] = []
    checked = 0
    for view, keys in served.items():
        fields = declared(source, view)
        if fields is None:
            # A view the client has not typed at all is not a drift, it is a
            # surface it does not use.
            continue
        allowed = set(keys) | set(CLIENT_ONLY.get(view, {}))
        # An interface that extends another inherits its parent's fields, and
        # the parent is checked in its own right.
        rel = extends(source, view)
        parent, omitted = rel if rel else (None, set())
        if parent and parent in served:
            allowed |= set(served[parent])
        # And the other way: a parent's OPTIONAL fields are legitimately served
        # only by the richer view its children describe. `MemberSummary` is a
        # row of `/members` and `MemberDetail extends` it, so `conferrable` and
        # `backers` -- both `?:` -- arrive on the detail and never on the row.
        for child in children.get(view, []):
            if child in served:
                allowed |= set(served[child])
        # A field a CHILD view serves under a different type overrides the
        # parent's reading for that child, and is compared there.
        served_type = dict(keys)
        checked += 1
        inherited: dict[str, str | None] = {}
        if parent:
            inherited = {k: v for k, v in (declared(source, parent) or {}).items() if k not in omitted}
        for f, want in {**inherited, **fields}.items():
            if f not in allowed:
                problems.append(f"  {view}.{f} -- declared by the client, served by no node")
                continue
            got = served_type.get(f)
            # "unknown" is a field two branches disagree about and "null" is one
            # nothing was serving in this fixture; neither is evidence, so
            # neither is compared. `want` is None where the declaration is
            # something this parser refuses to guess at.
            if want is None or got in (None, "unknown", "null"):
                continue
            if want != got:
                problems.append(
                    f"  {view}.{f} -- the client declares {want}, the node serves {got}"
                )

    if not problems:
        print(f"view-shape: {checked} client view types agree with what the node serves.")
        return 0

    print("view-shape: the client declares fields the node does not serve:\n", file=sys.stderr)
    print("\n".join(problems), file=sys.stderr)
    print(
        "\nEither the node should serve them, or they are a mechanism this ledger no longer "
        "runs and the client is promising it. Check `crates/node/src/serve/views.rs` before "
        "deleting anything: a field that reads `undefined` disables whatever is gated on it, "
        "silently.",
        file=sys.stderr,
    )
    return 1 if args.check else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
