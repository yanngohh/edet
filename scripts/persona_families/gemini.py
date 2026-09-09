"""The Google Gemini family.

Written from `ai.google.dev/gemini-api/docs/{text-generation,structured-output}`
rather than from memory: the `google-genai` package, `genai.Client()` reading
`GOOGLE_API_KEY` from the environment, `client.interactions.create` with
`input=`, the schema under `response_format`, and `interaction.output_text`.

Token usage is read defensively. The two guides show the call and not the
usage field names, and a wrong guess there would print a confident zero rather
than fail — so what is not documented is reported as absent.
"""

import json
import os

NAME = "gemini"

MODEL = "gemini-3.8-flash"


def credential():
    for var in ("GOOGLE_API_KEY", "GEMINI_API_KEY"):
        if os.environ.get(var):
            return var
    return None


def _tokens(interaction, *names):
    """The first of `names` this response actually carries, or zero.

    Read through `getattr` because the guides document the call and not the
    usage shape; a hard-coded field name that does not exist would silently
    report zero tokens for every call, which reads as "this family is free".
    """
    usage = getattr(interaction, "usage", None) or getattr(interaction, "usage_metadata", None)
    if usage is None:
        return 0
    for n in names:
        v = getattr(usage, n, None)
        if isinstance(v, int):
            return v
    return 0


def complete(system, user, schema):
    from google import genai

    client = genai.Client()
    interaction = client.interactions.create(
        model=MODEL,
        # No separate system channel is documented for `interactions.create`,
        # so the persona instruction leads the input rather than being dropped.
        input=f"{system}\n\n{user}",
        response_format={
            "type": "text",
            "mime_type": "application/json",
            "schema": schema,
        },
    )
    usage = {
        "input_tokens": _tokens(interaction, "input_tokens", "prompt_token_count"),
        "output_tokens": _tokens(interaction, "output_tokens", "candidates_token_count"),
        "model": MODEL,
    }
    return json.loads(interaction.output_text), usage
