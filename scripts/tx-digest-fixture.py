#!/usr/bin/env python3
r"""Keep the client's signing-digest fixture identical to what the node emits.

`ui/src/lib/txdigest.ts` reimplements `crates/node/src/block.rs::tx_digest` --
and the bincode encoding of `edet_state::Tx` under it -- in TypeScript, so that
a wallet computes what it signs instead of asking a node for it. A wallet that
asked would be handed whatever the node liked: the digest of a different
envelope, signed and submitted while the member looked at something else, and
`/tx/check` is no defence because the same node answers it.

That means two implementations of one canonical encoding, and two
implementations drift. The same answer as `proof-fixture.py`: make the oracle
GENERATED. This runs `cargo run -p edet-node --features serve --example
tx_digest_fixture` and replaces (or, with `--check`, diffs) the fixture block
delimited by the two banner comments in `tx-digest.test.ts`.

The example's own `match` is what keeps the alphabet covered: a new transaction
fails to compile there until somebody writes a vector for it, and the vitest
asserts the variant count as well.

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
TEST_FILE = REPO_ROOT / "ui/src/lib/__tests__/tx-digest.test.ts"

BEGIN = "// --------------------------------- fixtures from the Rust implementation ----\n"
END = "// ---------------------------------------------------------------- the pin ----\n"


def emitted() -> str:
    """The fixture block, straight from the normative implementation."""
    out = subprocess.run(
        ["cargo", "run", "--quiet", "-p", "edet-node", "--features", "serve", "--example", "tx_digest_fixture"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
    )
    if out.returncode != 0:
        sys.stderr.write(out.stderr)
        raise SystemExit(f"tx-digest-fixture: the example failed (exit {out.returncode})")
    return out.stdout.rstrip("\n") + "\n"


def split(source: str) -> tuple[str, str, str]:
    """(before, current block, after) around the delimited fixture region."""
    try:
        start = source.index(BEGIN) + len(BEGIN)
        end = source.index(END)
    except ValueError as exc:
        raise SystemExit(f"tx-digest-fixture: banner comments not found in {TEST_FILE}: {exc}")
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
        print(f"tx-digest-fixture: wrote the fixture block in {TEST_FILE}")
        return 0

    if current == fresh:
        print("tx-digest-fixture: the client's pinned digests match the node.")
        return 0
    sys.stdout.writelines(
        difflib.unified_diff(
            current.splitlines(keepends=True),
            fresh.splitlines(keepends=True),
            fromfile="tx-digest.test.ts (committed)",
            tofile="tx_digest_fixture example (normative)",
        )
    )
    print(
        "\ntx-digest-fixture: the client's pinned digests are stale -- run "
        "`just tx-digest-fixture`, check that the variant tags in "
        "ui/src/lib/txdigest.ts still mirror `edet_state::tx::Tx`'s declaration "
        "order, and commit the result.",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
