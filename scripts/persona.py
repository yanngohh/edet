#!/usr/bin/env python3
"""**The generator.** Personas producing narratives and copy, from two model
families of independent lineage, into a queue a hand compiles.

Run BY HAND. Never by `ci`, never by `just ci`, never with a model call in any
gate — a gate that calls a model is a gate whose verdict is a sample.

    python3 scripts/persona.py attack    --families anthropic,<second> --samples 2
    python3 scripts/persona.py objection --families anthropic,<second> --samples 2
    python3 scripts/persona.py copy      --families anthropic,<second>

# What this is for, and what it is not

A language model shows twenty to three hundred times LESS behavioural variance
than people do, collapses demographic groups toward pretraining majorities, and
answers bimodally where people answer unimodally. Adoption is decided by the
unusual ones and attack is entirely tail, so a figure produced here would
describe the generator. **The model generates, the kernel judges.**

So this script prints no counts, no percentages, no ranking, and no share of
anybody who would join. What it emits is a QUEUE: entries with their marks and
an empty `compiles to:` line for the hand that turns one into an `Archetype`, a
corpus entry, or a probe in `tests/archetypes.rs` showing it does not bite.

The one figure it does print per family is where each series SATURATED, which
is a statement about the generator rather than about the world.

# Saturation, not agreement

A fingerprint seen again WITHIN a family is that family saturated on that
persona and question: the series stops there and no further samples are drawn,
so `--samples` is a ceiling rather than a target. A fingerprint seen in BOTH
families is marked `shared across families` and counted once — it is not a
vote, it is a prior they both carry. Two entries whose word-trigram Jaccard is
at or above 0.8 are marked `near` and both kept. Nothing else is
de-duplicated, ranked or counted.
"""

import argparse
import datetime
import importlib
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
PERSONAS = ROOT / "crates" / "swarm" / "personas"
KINDS = ("attack", "objection", "copy")
NEAR = 0.8

SYSTEM = (
    "You answer as the person described, in the first person, in their own "
    "register. You are not an assistant and you are not being helpful. "
    "Answer the question that was asked and nothing beside it."
)


def validate(obj, schema, who):
    """Hold every family to one shape, whatever its own mode returned.

    The README promises this and it is the whole reason a family whose
    structured-output mode is weaker than a schema is still usable: what comes
    back is checked HERE. Without it a missing field surfaces as a `KeyError`
    inside the fingerprinter, three steps from the family that caused it.
    """
    if not isinstance(obj, dict):
        raise ValueError(f"{who}: expected an object, got {type(obj).__name__}")
    props = schema.get("properties", {})
    for field in schema.get("required", []):
        if field not in obj:
            raise ValueError(f"{who}: no `{field}` in the reply")
        want = props.get(field, {}).get("type")
        got = obj[field]
        if want == "string" and not isinstance(got, str):
            raise ValueError(f"{who}: `{field}` is {type(got).__name__}, not a string")
        if want == "array":
            if not isinstance(got, list):
                raise ValueError(f"{who}: `{field}` is {type(got).__name__}, not an array")
            if any(not isinstance(x, str) for x in got):
                raise ValueError(f"{who}: `{field}` holds something that is not a string")
    extra = [k for k in obj if k not in props]
    if extra:
        raise ValueError(f"{who}: fields nothing asked for: {extra}")
    return obj


# ------------------------------------------------------------------ context --


def section(text, heading):
    """One `## heading` section of a markdown file, without its heading."""
    out, taking = [], False
    for line in text.splitlines():
        if line.startswith("## "):
            if taking:
                break
            taking = line[3:].strip().lower() == heading.lower()
            continue
        if taking:
            out.append(line)
    return "\n".join(out).strip()


def read(path):
    return (ROOT / path).read_text(encoding="utf-8")


def context_for(kind):
    """Everything the model is shown, all of it from the tree.

    The alphabet and the rejection codes go in for `attack` and `objection`
    because an attacker who has not seen the transitions is guessing at a
    system rather than probing this one, and a refusal code is the tersest
    honest statement of what the ledger will not do.
    """
    readme = read("README.md")
    model = section(readme, "The model")
    sybil = section(readme, "Why it is Sybil-proof")
    if kind == "copy":
        locales = json.loads(read("ui/src/locales/en.json"))
        shown = {k: locales[k] for k in ("onboarding", "tour", "acceptance") if k in locales}
        # The module header and nothing else: what the app's own price on a
        # counterparty IS, which is the one place the client does more than
        # render what the ledger says.
        pricing = read("ui/src/lib/pricing.ts")
        header = pricing.split("*/", 1)[0] + "*/"
        return "\n\n".join(
            [
                "## What the community is",
                model,
                "## The strings the app shows you",
                json.dumps(shown, indent=2, ensure_ascii=False),
                "## How the app prices a counterparty",
                header,
            ]
        )
    return "\n\n".join(
        [
            "## The model",
            model,
            "## Why it is said to be Sybil-proof",
            sybil,
            "## Every transition there is",
            read("crates/state/src/tx.rs"),
            "## Every way it can refuse you",
            read("crates/state/src/errors.rs"),
        ]
    )


# -------------------------------------------------------------- fingerprints --


def normalise(entry, kind):
    """The narrative as a fingerprint reads it: lower case, punctuation and
    whitespace collapsed, the steps joined."""
    if kind == "attack":
        raw = entry["narrative"] + " " + " ".join(entry["steps"])
    elif kind == "objection":
        raw = entry["objection"] + " " + entry["would_need_to_see"]
    else:
        raw = entry["key"] + " " + entry["reading"] + " " + entry["misreading"]
    raw = raw.lower()
    raw = re.sub(r"[^a-z0-9 ]+", " ", raw)
    return re.sub(r"\s+", " ", raw).strip()


def trigrams(text):
    words = text.split()
    return {tuple(words[i : i + 3]) for i in range(max(0, len(words) - 2))}


def jaccard(a, b):
    if not a or not b:
        return 0.0
    return len(a & b) / len(a | b)


# ------------------------------------------------------------------- the run --


def load_family(name):
    try:
        return importlib.import_module(f"persona_families.{name}")
    except ModuleNotFoundError:
        return None


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("kind", choices=KINDS)
    ap.add_argument("--families", default="", help="two families of independent lineage, comma-separated")
    ap.add_argument("--samples", type=int, default=2, help="a CEILING per persona per family, not a target")
    args = ap.parse_args()

    sys.path.insert(0, str(ROOT / "scripts"))
    names = [n.strip() for n in args.families.split(",") if n.strip()]
    if len(names) < 2:
        print(
            "persona: two model families are required and one is refused.\n"
            "Draws from one family share one prior and converge on it, so a second sample of it\n"
            "buys repetition rather than a second opinion — and crossing populations is the only\n"
            "mechanism that ever reached what a single population shared.\n"
            "See scripts/persona_families/README.md for the contract; the requirement is an\n"
            "independent pretraining LINEAGE, not a brand.",
            file=sys.stderr,
        )
        return 2

    families = []
    for name in names:
        mod = load_family(name)
        if mod is None:
            print(f"no adapter for the family {name} — see scripts/persona_families/README.md", file=sys.stderr)
            return 2
        if mod.credential() is None:
            print(f"no credential for {name}, nothing was asked", file=sys.stderr)
            return 2
        families.append(mod)

    cards = json.loads((PERSONAS / "cards.json").read_text(encoding="utf-8"))
    schema = json.loads((PERSONAS / "schema.json").read_text(encoding="utf-8"))[args.kind]
    prompt = (PERSONAS / f"{args.kind}.md").read_text(encoding="utf-8")
    context = context_for(args.kind)

    entries = []
    saturation = {}
    usage = {f.NAME: {"input_tokens": 0, "output_tokens": 0, "calls": 0} for f in families}

    for card in cards:
        for fam in families:
            seen = set()
            for sample in range(1, args.samples + 1):
                user = prompt.replace("{{CARD}}", card["card"]).replace("{{CONTEXT}}", context)
                who = f"{fam.NAME}/{card['name']}/{sample}"
                try:
                    obj, used = fam.complete(SYSTEM, user, schema)
                    obj.setdefault("persona", card["name"])
                    validate(obj, schema, who)
                except Exception as e:  # noqa: BLE001 — a family that failed is a fact to record
                    print(f"  {who}: {e}", file=sys.stderr)
                    break
                usage[fam.NAME]["input_tokens"] += used.get("input_tokens", 0)
                usage[fam.NAME]["output_tokens"] += used.get("output_tokens", 0)
                usage[fam.NAME]["calls"] += 1
                fp = normalise(obj, args.kind)
                if fp in seen:
                    saturation[(fam.NAME, card["name"])] = sample
                    break
                seen.add(fp)
                entries.append(
                    {
                        "family": fam.NAME,
                        "sample": sample,
                        "persona": card["name"],
                        "obj": obj,
                        "fp": fp,
                        "grams": trigrams(fp),
                        "marks": [],
                    }
                )
                print(f"  {who}", file=sys.stderr)

    mark(entries)
    out = ROOT / f"PERSONA-{args.kind}-{datetime.date.today().isoformat()}.md"
    out.write_text(render(args, entries, saturation, usage, cards, families), encoding="utf-8")
    print(f"\n{out}")
    for fam in families:
        u = usage[fam.NAME]
        print(f"  {fam.NAME}: {u['calls']} calls, {u['input_tokens']} in, {u['output_tokens']} out")
        stopped = [f"{p} at {n}" for (f, p), n in sorted(saturation.items()) if f == fam.NAME]
        print(f"    saturated: {', '.join(stopped) if stopped else 'nowhere inside the ceiling'}")
    return 0


def mark(entries):
    """Shared across families, and near. Nothing else."""
    by_fp = {}
    for e in entries:
        by_fp.setdefault(e["fp"], []).append(e)
    for group in by_fp.values():
        if len({e["family"] for e in group}) > 1:
            for e in group:
                e["marks"].append("shared across families — a prior they both carry, not a vote")
    for i, a in enumerate(entries):
        for b in entries[i + 1 :]:
            if a["fp"] != b["fp"] and jaccard(a["grams"], b["grams"]) >= NEAR:
                a["marks"].append(f"near {b['family']}/{b['persona']}/{b['sample']}")
                b["marks"].append(f"near {a['family']}/{a['persona']}/{a['sample']}")


def render(args, entries, saturation, usage, cards, families):
    lines = [
        f"# Persona queue — {args.kind}",
        "",
        "A working document. It is not in the tree (`/PERSONA-*.md` is gitignored)",
        "and nothing in it is a measurement.",
        "",
        "**No number here is a measurement.** These are hypotheses from a generator whose",
        "variance is two orders of magnitude smaller than the phenomenon, and a hypothesis",
        "is worth exactly what the kernel says when it is compiled into a probe. The one",
        "figure that IS about something is where each series saturated, which is a",
        "statement about the generator.",
        "",
        "Work it entry by entry. `compiles to:` is for the hand: an `Archetype`, a corpus",
        "entry, or a `tests/archetypes.rs` probe SHOWING it does not bite — the last is a",
        "result too, and the one most of these will be.",
        "",
        "## Where each series stopped",
        "",
    ]
    for fam in families:
        u = usage[fam.NAME]
        lines.append(f"- **{fam.NAME}** — {u['calls']} calls, {u['input_tokens']} in, {u['output_tokens']} out")
        for card in cards:
            n = saturation.get((fam.NAME, card["name"]))
            where = f"saturated at sample {n}" if n else "did not saturate inside the ceiling"
            lines.append(f"  - {card['name']}: {where}")
    lines += [
        "",
        "A run in which NO series saturates inside the ceiling is a finding about the",
        "ceiling. One in which every series saturates at sample 1 is a finding about the",
        "prompt.",
        "",
        "## The queue",
        "",
    ]
    for e in entries:
        lines.append(f"### {e['persona']} — {e['family']}, sample {e['sample']}")
        lines.append("")
        for m in dict.fromkeys(e["marks"]):
            lines.append(f"> {m}")
        if e["marks"]:
            lines.append("")
        for k, v in e["obj"].items():
            if isinstance(v, list):
                lines.append(f"**{k}**")
                lines += [f"{i + 1}. {x}" for i, x in enumerate(v)]
            else:
                lines.append(f"**{k}** — {v}")
            lines.append("")
        lines.append("compiles to:")
        lines.append("")
    return "\n".join(lines) + "\n"


if __name__ == "__main__":
    sys.exit(main())
