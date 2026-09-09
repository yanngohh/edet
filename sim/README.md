# edet simulation

The independent half of the kernel cross-pin.

```sh
pip install -r requirements.txt      # numpy, scipy
python3 gen_fixtures.py              # regenerate fixtures/kernel.json
python3 run.py --fixtures            # the security probes + fixture freshness
```

## What's here

- **`edet_ref.py`** — the v0.7.0 capacity model, computed with
  `scipy.sparse.csgraph.maximum_flow`. The point is that this is **not the Rust
  kernel written twice**: capacity is a maximum flow, and scipy's is an
  implementation by people who had never heard of this project, so agreement
  says the kernel computes the right *mathematics* rather than that two copies
  of one algorithm agree with each other.
- **`gen_fixtures.py`** → **`fixtures/kernel.json`** — whole scenes (a stake
  graph, an underwriter set, a measured account set) with the answer scipy
  gives. `crates/kernel/tests/fixtures.rs` replays every one and demands
  **bit-identical** agreement.

  A cross-pin over floating-point fixed points
  and spectral radii to a tolerance; the whole capacity path is integer now, so
  two exact algorithms over the same integers have nothing to disagree about,
  and a tolerance here would be hiding something rather than accommodating
  anything.
- **`suites/security.py`** — the security claims of the paper's §Security, probed against the
  reference. Each asserts over a **set** where the claim is about a set: every
  defect this model has carried was a bound asserted over a set and tested over
  a singleton, and a per-account check passes in every one of them.

  Three of them were added by the paper pass, and the reason is worth
  keeping: the paper and the security table both carried the **collusion
  bound**, the **linearity of attack cost** and two **mixed-traffic** figures
  with *no probe anywhere in the tree* producing them. They were measured
  against a formulation of the model that was replaced three commits after the
  paper first stated them. Check that a claim's measurement EXISTS, not that its
  wording matches.

  The seat probe drives the reserve path on the WRITE layer, with the target's
  own supply arc in the network: a row is a stock, so its price is a
  reservation nothing releases, and the count one edge of 500.00 carries comes
  out at exactly `cut / bond unit` here as it does in the kernel. It is checked
  at every prefix as well as at the end, because a bound that holds only in
  aggregate is a bound tested at the wrong quantifier.

  The mixed-traffic probe is also the first thing here to drive the **reserve**
  path rather than the capacity query. `edet_ref.reserve` imposes the amount by
  capping the sink arc — a maximum flow truncated afterwards is not an
  augmentation of the size asked for — and splits each ledger edge through a node
  of its own, because scipy reports an antisymmetric flow matrix and would net two
  accounts that back each other, which is exactly the shape the wash probe builds.

## What is not here any more

An agent-based model, a trust fixed point, a reliability quantile and
the spectral radius of the contagion operator. None of them exists in v0.7.0 —
standing is a cut, and a cut has no fixed point to converge to and no operator
to take the radius of. Simulating them would have been simulating a design that
was replaced.

The strategy swarm in `crates/swarm` is not that returning. It is a population
over the REAL transition function, judged by the real audit — `edet_state::apply`
is what runs and `invariants::audit` is what decides — so it models nothing and
computes no fixed point. Every quantity it reports is read off `State`, `Member`
and the flow, which is why the sentence above stays true.
