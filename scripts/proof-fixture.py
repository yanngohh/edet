#!/usr/bin/env python3
r"""Keep the client's inclusion-proof fixture identical to what the node emits.

`ui/src/lib/proof.ts` reimplements `crates/state/src/root.rs` in TypeScript,
and its vitest pins a block of `RUST_*` constants that are supposed to be real
proofs from the normative implementation. Nothing read both, so the two drifted
in lockstep and the test stayed green through it:

  * `Section::ALL` lost its `pending` section, so the top tree went from six
    leaves to five;
  * `proof.ts` kept six, folding every `section_path` at the wrong shape --
    meaning the shipped client REFUSED every genuine proof the node served;
  * and `proof.test.ts` kept a fixture generated before the change, which its
    own six-section prover verified perfectly.

A cross-pin whose oracle is a hand-copied constant is pinned to whatever was
last pasted, which is the one failure a cross-pin exists to be immune to. This
script makes the fixture GENERATED: it runs
`cargo run -p edet-state --example proof_fixture` and replaces (or, with
`--check`, diffs) the fixture block delimited by the two banner comments in
`proof.test.ts`.

Two modes, matching `paper-constants.py`:
  * default: rewrite the block in place (`just proof-fixture`).
  * `--check`: byte-diff and exit 1 on drift (`just proof-fixture-check`, in
    `ci`).

Stdlib-only, so a gate that runs on every push cannot break on a dependency of
its own.
"""

from __future__ import annotations

import argparse
import difflib
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
TEST_FILE = REPO_ROOT / "ui/src/lib/__tests__/proof.test.ts"

# The block is delimited rather than located by regex over its contents: the
# fixture is opaque hex whose SHAPE changes with the format, so anything that
# tried to parse it would need updating for exactly the changes this gate is
# here to catch.
BEGIN = "// --------------------------------- fixtures from the Rust implementation ----\n"
END = "// --------------------------------------------------------------- the pin ----\n"


def emitted() -> str:
    """The fixture block, straight from the normative implementation."""
    out = subprocess.run(
        ["cargo", "run", "--quiet", "-p", "edet-state", "--example", "proof_fixture"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
    )
    if out.returncode != 0:
        sys.stderr.write(out.stderr)
        raise SystemExit(f"proof-fixture: the example failed (exit {out.returncode})")
    return out.stdout.rstrip("\n") + "\n"


def split(source: str) -> tuple[str, str, str]:
    """(before, current block, after) around the delimited fixture region."""
    try:
        start = source.index(BEGIN) + len(BEGIN)
        end = source.index(END)
    except ValueError as exc:
        raise SystemExit(f"proof-fixture: banner comments not found in {TEST_FILE}: {exc}")
    return source[:start], source[start:end], source[end:]


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="fail on drift instead of rewriting")
    args = parser.parse_args(argv)

    source = TEST_FILE.read_text(encoding="utf-8")
    before, current, after = split(source)
    fresh = "\n" + emitted() + "\n\n"

    if not args.check:
        TEST_FILE.write_text(before + fresh + after, encoding="utf-8")
        print(f"proof-fixture: wrote the fixture block in {TEST_FILE}")
        return 0

    if current == fresh:
        print("proof-fixture: the client's pinned fixture matches the node.")
        return 0
    sys.stdout.writelines(
        difflib.unified_diff(
            current.splitlines(keepends=True),
            fresh.splitlines(keepends=True),
            fromfile="proof.test.ts (committed)",
            tofile="proof_fixture example (normative)",
        )
    )
    print(
        "\nproof-fixture: the client's pinned proofs are stale -- run "
        "`just proof-fixture`, check that `SECTIONS`/`SECTION_TAG` in "
        "ui/src/lib/proof.ts still mirror `Section::ALL`, and commit the result.",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
