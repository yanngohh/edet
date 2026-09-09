"""The OpenAI family.

Written from `developers.openai.com/api/docs/guides/{text,structured-outputs}`
rather than from memory: the Responses API (`client.responses.create`), the
system half as `instructions=` beside `input=`, the schema under
`text.format` with `type: "json_schema"`, and `response.output_text` for the
aggregated text.
"""

import json
import os

NAME = "openai"

# The reasoning model the current guides use in their own examples. Named here
# rather than buried in the call so that changing family strength is one line.
MODEL = "gpt-6-astra"


def credential():
    return "OPENAI_API_KEY" if os.environ.get("OPENAI_API_KEY") else None


def complete(system, user, schema):
    from openai import OpenAI

    client = OpenAI()
    response = client.responses.create(
        model=MODEL,
        instructions=system,
        input=user,
        text={
            "format": {
                "type": "json_schema",
                "name": "persona_entry",
                "schema": schema,
                "strict": True,
            }
        },
    )
    usage = {
        "input_tokens": getattr(response.usage, "input_tokens", 0),
        "output_tokens": getattr(response.usage, "output_tokens", 0),
        "model": getattr(response, "model", MODEL),
    }
    return json.loads(response.output_text), usage
