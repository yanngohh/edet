"""The Mistral family.

Written from `docs.mistral.ai` and the SDK's own README rather than from
memory: `from mistralai.client import Mistral`, `chat.complete`, and
`res.choices[0].message.content`.

**JSON mode rather than a schema, deliberately.** The plain
`{"type": "json_object"}` form is the one the quickstart documents; the
schema form is not shown in what those pages carry, and a guessed parameter
would fail at the provider rather than here. Nothing is lost: the driver
validates every family's reply against `personas/schema.json` whatever the
adapter returns, so a family whose mode is weaker than a schema is still held
to the same shape — and the schema goes into the prompt so the model is asked
for it rather than left to invent a shape.
"""

import json
import os

NAME = "mistral"

MODEL = "mistral-large-latest"


def credential():
    return "MISTRAL_API_KEY" if os.environ.get("MISTRAL_API_KEY") else None


def _tokens(res, *names):
    usage = getattr(res, "usage", None)
    if usage is None:
        return 0
    for n in names:
        v = getattr(usage, n, None)
        if isinstance(v, int):
            return v
    return 0


def complete(system, user, schema):
    from mistralai.client import Mistral

    asked = f"{user}\n\nAnswer as JSON matching this schema exactly:\n{json.dumps(schema, indent=2)}"
    with Mistral(api_key=os.environ["MISTRAL_API_KEY"]) as client:
        res = client.chat.complete(
            model=MODEL,
            messages=[{"role": "system", "content": system}, {"role": "user", "content": asked}],
            response_format={"type": "json_object"},
        )
    usage = {
        "input_tokens": _tokens(res, "prompt_tokens", "input_tokens"),
        "output_tokens": _tokens(res, "completion_tokens", "output_tokens"),
        "model": MODEL,
    }
    return json.loads(res.choices[0].message.content), usage
