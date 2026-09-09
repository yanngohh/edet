"""The Anthropic family.

Written from the `/claude-api` skill's Python pages rather than from memory:
`client.beta.messages.stream(...)` with `get_final_message()` because a long
structured answer is exactly the shape that hits an HTTP timeout without it,
`output_config={"format": {"type": "json_schema", ...}}` for the structured
output, adaptive thinking left at its default, and the server-side refusal
`fallbacks` the skill says to include by default on this model.
"""

import json
import os

NAME = "anthropic"

MODEL = "claude-opus-5"
MAX_TOKENS = 16000


def credential():
    """What would authenticate this family, or `None`.

    An unset `ANTHROPIC_API_KEY` does not mean there are no credentials: the
    SDK also resolves `ANTHROPIC_AUTH_TOKEN` and an `ant auth login` profile,
    and a zero-argument client picks either up. So this reports the first
    thing that would work rather than asserting the environment variable.
    """
    for var in ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"):
        if os.environ.get(var):
            return var
    home = os.path.expanduser("~/.config/anthropic")
    if os.path.isdir(home) and os.listdir(home):
        return "an `ant auth login` profile"
    return None


def complete(system, user, schema):
    import anthropic

    client = anthropic.Anthropic()
    with client.beta.messages.stream(
        model=MODEL,
        max_tokens=MAX_TOKENS,
        betas=["server-side-fallback-2026-07-01"],
        fallbacks="default",
        system=system,
        messages=[{"role": "user", "content": user}],
        output_config={"format": {"type": "json_schema", "schema": schema}},
    ) as stream:
        message = stream.get_final_message()

    if message.stop_reason == "refusal":
        raise RuntimeError(
            "the whole fallback chain declined this prompt: "
            f"{getattr(message.stop_details, 'category', None)}"
        )
    text = next(b.text for b in message.content if b.type == "text")
    usage = {
        "input_tokens": message.usage.input_tokens,
        "output_tokens": message.usage.output_tokens,
        "model": message.model,
    }
    return json.loads(text), usage
