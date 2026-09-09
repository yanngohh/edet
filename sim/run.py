#!/usr/bin/env python3
"""Run the edet v0.7.0 security probes and check the fixtures are current.

  python3 sim/run.py              # the probes
  python3 sim/run.py --fixtures   # and that fixtures/kernel.json is not stale

Nonzero exit on any failure, so CI can gate on it.
"""

from __future__ import annotations

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).parent))
sys.path.insert(0, str(pathlib.Path(__file__).parent / "suites"))

import gen_fixtures  # noqa: E402
import security  # noqa: E402


def main():
    print("security probes (against an independent max-flow)")
    failures = security.run()

    if "--fixtures" in sys.argv:
        path = pathlib.Path(__file__).parent / "fixtures" / "kernel.json"
        current = json.dumps(gen_fixtures.build(), indent=2) + "\n"
        stale = not path.exists() or path.read_text() != current
        print(f"\n  {'FAIL' if stale else 'PASS'}  fixtures/kernel.json is current")
        if stale:
            print("        run: python3 sim/gen_fixtures.py")
        failures += stale

    print(f"\n{'FAILED' if failures else 'ok'}: {failures} failure(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
