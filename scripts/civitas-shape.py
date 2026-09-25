#!/usr/bin/env python3
r"""Check the civitas player's reads against what `edet-civitas index` writes.

The player (`ui/player/src`) reads the JSON `index` writes under
`<run>/player/` with plain property access, and nothing reads both sides. A
field the player reads that `index` no longer writes does not fail anywhere: it
reads `undefined`, a figure shows as a dash or nothing, and a watcher is shown
a community that did not happen. It is the failure `just view-shape-check`
exists for between the wallet and the node, one tool over.

What it does: asks `edet-civitas shape` for every key `index` can write —
built from `index`'s own functions over one event of every kind, so a field
that appears only on a violation or a silent day is in it — collects every
lower-case property name the player's sources access, and fails on a name that
is neither one of those keys, a key of an object the player builds itself, nor
a JavaScript or browser property listed below. A key the player builds that the
index also writes is itself a failure, since it would hide that field going
missing.

What it cannot see: a field read by bracket notation (`row["capacity"]`) or by
destructuring (`const { capacity } = row`). The player reads the index by dot
access only; keep it that way, or this check stops covering what it reads.

It is a text scanner, not a JavaScript parser, and it misreads four constructs
the player does not use today: a regex literal holding `//` or a pair of
quotes, a template literal nested in another's `${...}`, a `}` inside a
`${...}`, and two apostrophes on one line of markup text. Each can hide a read
from it; keep them out of the player's sources, or read the report twice.

Direction, as in `view-shape.py`: this fails on a field the PLAYER reads and
`index` does not write. `index` writing something no view reads yet is a
surface, not a defect.

Run BY HAND (`just civitas-shape-check`), never by `ci`: nothing of the
simulation enters a gate.

Two modes:
  * default: report.
  * `--check`: exit 1 on drift.

Stdlib-only.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
PLAYER_SRC = REPO_ROOT / "ui/player/src"

# Property names the player reads that are not the index's: JavaScript and the
# DOM. Keep this list to names that cannot be a field the player reads from the
# index; `key` is one the index writes (a person's key) and the player reads
# only off a key press, so a lost person key would not be caught here.
NOT_FIELDS: set[str] = {
    # arrays and strings
    "length",
    # Math
    "pi",
    # the DOM and its events: `e.target.files`, `input.value`, a rectangle's
    # `left` and `width`, a pointer's `clientX`, a component event's `detail`,
    # a key press's `key`, a `<details>`'s `open`
    "target", "files", "value", "currenttarget", "left", "width", "clientx", "detail", "key", "open",
    "webkitrelativepath",
    # an Error's `message`
    "message",
    # a `fetch` Response: the player now reads runs the dev server offers from
    # the runs directory beside the tree, and `res.ok` is the HTTP verdict, not
    # a field of a tape.
    "ok", "status",
    # the defaults register the player derives from `expired_today`
    # (`analytics.defaultsRegister`): when a marked default was resolved,
    # whether the same day, and whether an underwriter took the claim over
    "steps", "marked", "what",
    # the browser's own vocabulary, which reading names in ANY case exposed:
    # keyboard and pointer events, the canvas's drawing state, the observers.
    "altkey", "ctrlkey", "metakey", "shiftkey", "button", "clienty", "deltay",
    "pointerid", "dataset", "documentelement", "devicepixelratio", "matches",
    "contentrect", "isintersecting", "hidden", "height", "width", "top",
    "fillstyle", "strokestyle", "linewidth", "linejoin", "textalign", "font", "radius",
    "size",
    # the tooltip the player draws: an element's `classList`, a box's `bottom`,
    # the window's `innerWidth` it is kept inside of
    "classlist", "innerwidth", "innerheight", "bottom",
    # `dataset.theme` is the player's own switch, written onto the document;
    # `p.m` is the member a projected point was made from.
    "theme", "m",
}


def emitted() -> set[str]:
    """Every key the index can write, from the normative implementation."""
    out = subprocess.run(
        ["cargo", "run", "--quiet", "-p", "edet-civitas", "--", "shape"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
    )
    if out.returncode != 0:
        sys.stderr.write(out.stderr)
        raise SystemExit(f"civitas-shape: `edet-civitas shape` failed (exit {out.returncode})")
    keys: set[str] = set()
    # `shape` answers `{run, day, life}`: one value per file kind, whose own
    # keys are what the index writes. The three names are the wrapper's.
    for value in json.loads(out.stdout).values():
        keys_of(value, keys)
    return keys


def keys_of(value, into: set[str]) -> None:
    if isinstance(value, dict):
        for k, v in value.items():
            into.add(k)
            keys_of(v, into)
    elif isinstance(value, list):
        for v in value:
            keys_of(v, into)


def scripts_of(source: str) -> str:
    """The script half of a source: a `.js` file whole, a `.svelte` file's
    `<script>` blocks."""
    blocks = re.findall(r"<script[^>]*>([\s\S]*?)</script>", source)
    return "\n".join(blocks) if blocks else source


def code_only(source: str) -> str:
    """The source with styles, comments and string contents removed, keeping a
    template literal's `${...}` expressions: what is left is what runs.

    Strings, template literals and comments are matched in ONE pass, so
    whichever starts first wins: a `//` inside a URL string is part of the
    string, and a quote inside a comment is part of the comment."""
    source = re.sub(r"<style[\s\S]*?</style>", "", source)
    token = re.compile(
        r"(?P<dq>\"(?:[^\"\\\n]|\\.)*\")"
        r"|(?P<sq>'(?:[^'\\\n]|\\.)*')"
        r"|(?P<tl>`(?:[^`\\]|\\.)*`)"
        r"|(?P<lc>//[^\n]*)"
        r"|(?P<bc>/\*[\s\S]*?\*/)"
    )

    def keep(m: re.Match) -> str:
        if m.group("tl"):
            return " ".join(re.findall(r"\$\{([^}]*)\}", m.group("tl")))
        if m.group("dq") or m.group("sq"):
            return '""'
        return ""

    return token.sub(keep, source)


def own_keys(source: str) -> set[str]:
    """Keys of objects the player builds itself — `{ label: ..., g: true }`,
    a returned `{ days, silent, ... }` — which it may read back freely. Read
    from code only: a comment describing a shape must not exempt a field."""
    script = code_only(scripts_of(source))
    keys = set(re.findall(r"[{,]\s*([A-Za-z_][A-Za-z0-9_]*)\s*:(?!:)", script))
    for body in re.findall(r"(?:return|=>?)\s*\{([^{}]*)\}", script):
        keys |= set(re.findall(r"(?:^|,)\s*([A-Za-z_][A-Za-z0-9_]*)\s*(?=,|$)", body.strip()))
    return keys


def imports(source: str) -> set[str]:
    """The player's own modules a source imports, by file name."""
    return {
        m.group(1)
        for m in re.finditer(r"""from\s+['"]\./([A-Za-z0-9_.-]+)['"]""", scripts_of(source))
    }


def read(source: str) -> set[str]:
    """Names the source accesses as `x.name` or `x?.name`.

    Any case: a pattern stopping at the first upper-case letter matches
    NOTHING of `.priceIndex` — not the whole name and not its head — so a
    camel-cased read of a field the index stopped writing would pass a check
    whose whole promise is to catch exactly that."""
    source = code_only(source)
    names = set()
    # The receiver is a lookbehind, not part of the match: a chain such as
    # `a.result.cash_moved` would otherwise consume `result` as the receiver of
    # one match and never see `.cash_moved` as a read.
    # A call is never a read of a field: JSON holds no functions.
    for m in re.finditer(r"(?<=[\w$)\]])\??\.([A-Za-z_][A-Za-z0-9_]*)\b(?!\s*\()", source):
        names.add(m.group(1))
    return names


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--check", action="store_true", help="exit 1 on drift")
    args = parser.parse_args(argv)

    keys = emitted()
    problems: list[str] = []
    files = sorted(PLAYER_SRC.glob("*.svelte")) + sorted(PLAYER_SRC.glob("*.js"))
    # A file that AUTHORS a run — the demo — reads nothing from a real one, so
    # its reads are nothing to check; what it BUILDS is still the player's own
    # vocabulary and counts as such.
    authored = {"demo.js"}
    # **What the player builds, it may read** — a view object made in one module
    # is read in another, and a component reads what its props were built from,
    # which no import graph shows. So the exemption is the union over the
    # player's real sources.
    #
    # The DEMO is the exception, and the reason this is not one union: its reads
    # are not checked at all, so an object literal in it was silencing the same
    # name read anywhere else in the player. Its keys are exempt only in the
    # files that actually import it.
    own_by_file = {f.name: own_keys(f.read_text(encoding="utf-8")) for f in files}
    imports_by_file = {f.name: imports(f.read_text(encoding="utf-8")) for f in files}
    own: set[str] = set()
    for name, keys_of_file in own_by_file.items():
        if name not in authored:
            own |= keys_of_file
    # An object the player builds with a key the index also writes would hide
    # that field's disappearance: a read of it would still find a key. Such a
    # key is renamed in the player rather than trusted here.
    # **A collision is a warning and no longer a verdict.** When this was
    # written the player only READ the index; it now also authors a demo run
    # in the index's own shape and derives series over it, so it builds
    # objects whose fields are the same vocabulary — `capacity`, `insured`,
    # `from`, `to` — and must. What is still a verdict is the read: a field
    # the player reads that neither the index writes nor the player builds.
    # A collision can hide a disappearance, so it is still said out loud.
    shadowed = sorted(own & keys)
    for f in files:
        if f.name in authored:
            continue
        mine = own - keys
        for name in authored & imports_by_file.get(f.name, set()):
            mine |= own_by_file.get(name, set()) - keys
        for name in sorted(read(f.read_text(encoding="utf-8"))):
            if name in keys or name in mine or name.lower() in NOT_FIELDS:
                continue
            problems.append(f"{f.relative_to(REPO_ROOT)}: reads `.{name}`, which `index` does not write")

    if shadowed:
        print(f"civitas-shape: {len(shadowed)} name(s) the player builds are also written by `index`:")
        print(f"  {', '.join(shadowed)}")
        print("  a read of one of those cannot tell the two apart, so it would not notice the index dropping it")
    if problems:
        print("civitas-shape: the player reads fields the index does not write:")
        for p in problems:
            print(f"  {p}")
        return 1 if args.check else 0
    print(f"civitas-shape: {len(files)} player source(s) read only what `index` writes ({len(keys)} keys).")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
