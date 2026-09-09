#!/usr/bin/env python3
r"""Generate the paper's genesis-constant macros from the kernel's source of truth.

`crates/kernel/src/constants.rs` is where a genesis value is DECIDED; the paper
only ever CITES one. Before this script existed those were two hand-maintained
copies with no gate between them, which is exactly the shape of drift that
survives review: a constant changes for a protocol reason, the commit touches
one file, and the paper keeps printing the old number forever after, silently,
because nothing ever reads both.

This script parses every `pub const NAME: TYPE = VALUE;` line in the kernel
constants module and emits one `\newcommand` per constant into
`paper/sections/generated-constants.tex`, which `paper/edet.tex`'s preamble
`\input`s. The prose then cites `\kConstAlpha` etc. instead of typing `0.15` --
so a kernel change and a paper rebuild can never again show two different
numbers for the same constant; they show the same macro, expanded from the
same source.

Two modes:
  * default: regenerate `generated-constants.tex` in place.
  * `--check`: regenerate to memory, byte-diff against the committed file, and
    exit 1 with a unified diff if it drifted. This is the CI gate
    (`just paper-constants-check`) -- it fails loudly on a push that changed a
    constant and forgot to regenerate, rather than letting the paper go stale
    quietly, which is the failure this script exists to close.

Deliberately stdlib-only (`re`, `difflib`, `pathlib`, `argparse`): this runs in
`just ci` on every push, so it must not gain a new dependency of its own to
break.
"""

from __future__ import annotations

import argparse
import difflib
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CONSTANTS_RS = REPO_ROOT / "crates" / "kernel" / "src" / "constants.rs"
GENERATED_TEX = REPO_ROOT / "paper" / "sections" / "generated-constants.tex"

# Matches exactly the shape `constants.rs` uses: one constant per line, no
# multi-line declarations, no expressions beyond a literal or a bare reference
# to an earlier constant (the alias form, e.g. `SIGMA_EST: f64 = BASE_CAPACITY;`).
# Anything else in the file (doc comments, blank lines, the module doc) simply
# does not match and is skipped -- this is a parser for one narrow shape, on
# purpose, so a future constant the two forms below can't express fails loudly
# in `_parse` rather than being silently mis-rendered.
CONST_LINE = re.compile(
    r"^pub const (?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*:\s*"
    r"(?P<ty>[A-Za-z0-9_<>:]+)\s*=\s*(?P<value>[^;]+);\s*$"
)
# A numeric literal, Rust-style: optional sign, digits (with `_` separators
# allowed anywhere, exactly as the compiler allows), optional fractional part.
NUMERIC_LITERAL = re.compile(r"^-?[0-9][0-9_]*(\.[0-9][0-9_]*)?$")
# A bare identifier: the const-to-const alias form (`SIGMA_EST = BASE_CAPACITY`).
ALIAS_REF = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")

DIGIT_WORDS = {
    "0": "Zero",
    "1": "One",
    "2": "Two",
    "3": "Three",
    "4": "Four",
    "5": "Five",
    "6": "Six",
    "7": "Seven",
    "8": "Eight",
    "9": "Nine",
}

# LaTeX control words may only contain letters -- a digit ends the command
# name silently, so `\kConstW0Anchor` is not a typo waiting to happen, it is a
# compile-time impossibility. `W0_ANCHOR`, `A0`, and `B0` all carry a digit in
# their Rust name, so every digit run is spelled out (`0` -> `Zero`) rather
# than dropped, which keeps the macro name a lossless, unambiguous function of
# the constant name instead of one that collides two different constants onto
# the same letters-only macro.
_ALNUM_RUN = re.compile(r"[A-Za-z]+|[0-9]+")


def macro_name(rust_name: str) -> str:
    """`ALPHA` -> `\\kConstAlpha`; `W0_ANCHOR` -> `\\kConstWZeroAnchor`."""

    def word(run: str) -> str:
        if run.isdigit():
            return "".join(DIGIT_WORDS[d] for d in run)
        return run.capitalize()

    pieces = []
    for part in rust_name.split("_"):
        pieces.append("".join(word(run) for run in _ALNUM_RUN.findall(part)))
    return "\\kConst" + "".join(pieces)


# The grouping this mirrors already exists once in the paper by hand
# (`T_{\mathrm{epoch}} = 86\,400\,\mathrm{s}` in 20-model.tex): a LaTeX thin
# space every three digits from the right, on the integer part only. It is
# trivial to do generically with Python's own `,`-grouping format spec, so
# rather than special-case 86_400 and leave every other four-and-more-digit
# genesis value (37_000, 10_000, 1_000_000, ...) printed as one unbroken run of
# digits, every one of them gets the same treatment -- one rule, not an
# exception for the constant that happened to already appear in prose.
def _group_thousands(digits: str) -> str:
    sign = ""
    if digits.startswith("-"):
        sign, digits = "-", digits[1:]
    return sign + format(int(digits), ",").replace(",", "\\,")


def format_value(raw: str) -> str:
    """Render a Rust numeric literal the way the paper prints a genesis value:
    underscores dropped, trailing fractional zeros trimmed (`0.90` -> `0.9`,
    `30.0` -> `30`), and the integer part thousands-grouped with a LaTeX thin
    space once it reaches four digits (`86400` -> `86\\,400`)."""
    text = raw.replace("_", "")
    if "." in text:
        int_part, frac_part = text.split(".", 1)
        frac_part = frac_part.rstrip("0")
        int_part = _group_thousands(int_part)
        return f"{int_part}.{frac_part}" if frac_part else int_part
    return _group_thousands(text)


class ConstantsParseError(RuntimeError):
    pass


def parse_constants(source: str) -> list[tuple[str, str]]:
    """Return `[(NAME, formatted_value), ...]` in declaration order.

    Aliases (`VETO_WINDOW_EPOCHS = MIN_MATURITY_EPOCHS`) resolve to the
    referent's own formatted value -- textually, from the referent's already-
    parsed literal, never by round-tripping through `float()`. Floats lose
    trailing-zero information `float()` can't get back (`30.0` and `30` are
    the same `f64`), and this script's whole job is to reproduce the digits
    the source actually wrote, not a value merely equal to them.
    """
    resolved: dict[str, str] = {}
    ordered: list[tuple[str, str]] = []
    for lineno, line in enumerate(source.splitlines(), start=1):
        m = CONST_LINE.match(line.strip())
        if not m:
            continue
        name, value = m.group("name"), m.group("value").strip()
        if NUMERIC_LITERAL.match(value):
            formatted = format_value(value)
        elif ALIAS_REF.match(value) and value in resolved:
            formatted = resolved[value]
        else:
            raise ConstantsParseError(
                f"constants.rs:{lineno}: `{name} = {value}` is neither a "
                "numeric literal nor a reference to an already-declared "
                "constant -- teach parse_constants this shape or simplify "
                "the constant."
            )
        resolved[name] = formatted
        ordered.append((name, formatted))
    if not ordered:
        raise ConstantsParseError(f"no `pub const` declarations found in {CONSTANTS_RS}")
    return ordered


def render(constants: list[tuple[str, str]]) -> str:
    lines = [
        "% Auto-generated by scripts/paper-constants.py from",
        "% crates/kernel/src/constants.rs. Do not hand-edit: change the kernel",
        "% constant and run `just paper-constants`. `just paper-constants-check`",
        "% (part of `just ci`) fails the build if this file drifts from the",
        "% source it was generated from.",
        "",
    ]
    for name, value in constants:
        lines.append(f"\\newcommand{{{macro_name(name)}}}{{{value}}}")
    lines.append("")
    return "\n".join(lines)


def generate() -> str:
    source = CONSTANTS_RS.read_text(encoding="utf-8")
    return render(parse_constants(source))


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="regenerate to memory and byte-diff against the committed file "
        "instead of writing it; exit 1 on drift",
    )
    args = parser.parse_args(argv)

    try:
        content = generate()
    except ConstantsParseError as exc:
        print(f"paper-constants: {exc}", file=sys.stderr)
        return 1

    if args.check:
        if not GENERATED_TEX.exists():
            print(
                f"paper-constants: {GENERATED_TEX} does not exist -- run "
                "`just paper-constants` and commit it.",
                file=sys.stderr,
            )
            return 1
        committed = GENERATED_TEX.read_text(encoding="utf-8")
        if committed == content:
            print(f"paper-constants: {GENERATED_TEX} is up to date.")
            return 0
        diff = difflib.unified_diff(
            committed.splitlines(keepends=True),
            content.splitlines(keepends=True),
            fromfile=f"{GENERATED_TEX} (committed)",
            tofile=f"{GENERATED_TEX} (regenerated from constants.rs)",
        )
        sys.stdout.writelines(diff)
        print(
            "\npaper-constants: generated-constants.tex is stale -- run "
            "`just paper-constants` and commit the result.",
            file=sys.stderr,
        )
        return 1

    GENERATED_TEX.write_text(content, encoding="utf-8")
    print(f"paper-constants: wrote {GENERATED_TEX}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
