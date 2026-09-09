"""Model families, one adapter each.

**Two families of independent lineage, and the driver refuses one.** Draws
from a single family share one prior and converge on it, so a second sample of
the same family measures the sampler rather than the population — and crossing
populations is the only mechanism in that literature that ever reached what a
single population shared.

Every adapter exposes exactly:

    NAME: str
    def credential() -> str | None:   # what resolved, or None
    def complete(system: str, user: str, schema: dict) -> tuple[dict, dict]

returning `(the parsed object, a usage dict)`. The driver validates whatever
comes back against `personas/schema.json` itself, so an adapter that returns
something else is caught here rather than downstream.
"""
