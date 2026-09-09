#!/usr/bin/env python3
"""The npm half of `just audit`, made to fail for one reason at a time.

`npm audit` asks the registry's BULK advisory endpoint, and when that request
fails for any reason — a blip, a 5xx, a timeout — npm silently retries against
the legacy `/-/npm/v1/security/audits/quick` endpoint, which npm itself now
answers with `400 Bad Request` and a notice that it is being retired. The
command then exits non-zero. So the gate had two ways to go red that a reader
cannot tell apart from the exit code: a real high-or-critical advisory in the
dependency tree, and a transient failure to reach a service. Measured on
Measured: three consecutive runs of the identical command read bulk-200,
quick-400, bulk-200.

**A gate that goes red for two unrelated reasons teaches you to ignore it**,
which is the one thing this tree's gates may not do. So this asks in JSON,
retries when the answer is not a report, and distinguishes the outcomes in the
exit status and in the words:

  0  the registry answered and nothing runtime-facing is at or above the floor
  1  the registry answered and something is — the dependency tree is the problem
  2  the registry never answered — the tree is NOT implicated, and nothing here
     has been checked

Exit 2 is still a failure, deliberately: the recipe's own note has said since it
was written that this gate reads a live registry and can go red with no commit
behind it, and the answer to that is to say so plainly rather than to go green
on a claim nobody verified.

Two queries, not the three the shell version made — the dev-inclusive report is
a superset of the runtime one, so the three sections below are rendered from
them rather than re-fetched.
"""

import argparse
import json
import subprocess
import sys
import time

# Ascending, so a floor of "high" admits "critical" without naming it.
SEVERITIES = ["info", "low", "moderate", "high", "critical"]


def audit(prefix: str, omit_dev: bool, attempts: int, label: str) -> dict:
    """One audit report, retried while the registry gives something else.

    npm exits non-zero merely for FINDING vulnerabilities, so the exit code
    says nothing about whether the request worked; the presence of `metadata`
    is what separates a report from an error object.
    """
    cmd = ["npm", "--prefix", prefix, "audit", "--json"]
    if omit_dev:
        cmd.append("--omit=dev")
    for attempt in range(1, attempts + 1):
        # A hang is a failure to reach the registry like any other, and it must
        # arrive here rather than as a traceback: an uncaught exception exits 1,
        # which is the "the tree is at fault" code this exists to keep separate.
        try:
            out = subprocess.run(cmd, capture_output=True, text=True, timeout=180)
            stdout = out.stdout
        except subprocess.TimeoutExpired:
            stdout, detail = "", "timed out"
        except FileNotFoundError:
            # Not a registry problem and not a tree problem: nothing ran at all.
            raise EnvironmentError("npm is not on PATH") from None
        else:
            detail = ""
        try:
            report = json.loads(stdout)
        except json.JSONDecodeError:
            report = None
        if isinstance(report, dict) and "metadata" in report:
            return report
        if isinstance(report, dict) and "error" in report:
            err = report["error"]
            detail = err.get("summary") or err.get("code") or detail
        print(f"  {label}: no report on attempt {attempt}/{attempts} {detail}".rstrip(), flush=True)
        if attempt < attempts:
            time.sleep(2 * attempt)
    raise LookupError(label)


def counts(report: dict) -> dict:
    return report.get("metadata", {}).get("vulnerabilities", {})


def at_or_above(report: dict, floor: str) -> list:
    """Advisories at or above `floor`, as (name, severity, titles), sorted."""
    wanted = set(SEVERITIES[SEVERITIES.index(floor):])
    rows = []
    for name, v in report.get("vulnerabilities", {}).items():
        if v.get("severity") not in wanted:
            continue
        titles = sorted({e["title"] for e in v.get("via", []) if isinstance(e, dict) and "title" in e})
        rows.append((name, v.get("severity"), titles))
    return sorted(rows)


def render(rows: list) -> None:
    for name, severity, titles in rows:
        print(f"  {severity:>8}  {name}")
        for t in titles:
            print(f"            {t}")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--prefix", default="ui")
    ap.add_argument("--fail-at", default="high", choices=SEVERITIES)
    ap.add_argument("--attempts", type=int, default=3)
    args = ap.parse_args()

    try:
        runtime = audit(args.prefix, True, args.attempts, "runtime")
        tooling = audit(args.prefix, False, args.attempts, "dev/build")
    except EnvironmentError as why:
        print(f"\nUNRESOLVED: {why}. Nothing here has been checked.", file=sys.stderr)
        return 2
    except LookupError as which:
        print(
            f"\nUNRESOLVED: the npm advisory registry did not return a report for the "
            f"{which} tree in {args.attempts} attempts.\n"
            "This says nothing about the dependency tree — nothing here has been "
            "checked. It is not a reason to commit.",
            file=sys.stderr,
        )
        return 2

    breaching = at_or_above(runtime, args.fail_at)
    print(f"--- npm runtime dependencies ({args.fail_at} and above fail the gate) ---")
    if breaching:
        render(breaching)
    else:
        print(f"  none at or above {args.fail_at}")

    print("--- npm runtime, below the floor (reported only) ---")
    below = [r for r in at_or_above(runtime, "info") if r not in breaching]
    render(below) if below else print("  none")

    # The dev-inclusive report is a superset; what is left after removing the
    # runtime rows is exactly what only the build tooling pulls in.
    print("--- npm dev/build tooling only (reported only) ---")
    runtime_names = {name for name, _, _ in at_or_above(runtime, "info")}
    dev_only = [r for r in at_or_above(tooling, "info") if r[0] not in runtime_names]
    render(dev_only) if dev_only else print("  none")

    print(f"  totals: runtime {counts(runtime)}")
    print(f"          with dev {counts(tooling)}")
    return 1 if breaching else 0


if __name__ == "__main__":
    sys.exit(main())
