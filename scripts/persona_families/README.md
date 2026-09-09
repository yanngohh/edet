# The families

`persona.py` refuses to run on ONE family and says so, because the whole design
of the persona layer rests on crossing them. Four are shipped —
`anthropic`, `gemini`, `mistral`, `openai` — and more than two is strictly
better for the same reason two beats one.

    python3 scripts/persona.py attack --families anthropic,gemini,mistral

**Each adapter is written from that provider's documented API, verified
against it rather than recalled**, and the model id sits at the top of its file
so changing a family's strength is one line. Where a provider's structured mode
is weaker than a schema — Mistral's documented JSON mode names no schema — the
adapter uses what is documented and puts the schema in the prompt; nothing is
lost, because `persona.py::validate` holds EVERY family to
`personas/schema.json` whatever its adapter returned.

**The requirement is an independent pretraining lineage, not a brand.** A
distillation of the first family is the first family: it carries the same
prior, so a "second opinion" from it is the first opinion with more variance.
Two draws of one family buy repetition.

To add another, write `scripts/persona_families/<name>.py` against that
provider's DOCUMENTED API — verified against it, not recalled — exposing
exactly what `__init__.py` names:

```python
NAME = "<name>"

def credential():
    """The env var or profile that would authenticate, or None."""

def complete(system, user, schema):
    """-> (parsed object, {"input_tokens": int, "output_tokens": int, "model": str})

    Use that provider's own structured-output or JSON mode. The driver
    validates the result against `personas/schema.json` whatever the adapter
    returns, so a family whose mode is weaker than a schema is still held to
    the same shape.
    """
```

Then add its official client to `scripts/persona-requirements.txt`. **Each
family speaks through its own official client, never through a compatibility
shim for the other** — a shim makes the second family's outputs a function of
the first family's API surface, which is one more thing they share.
