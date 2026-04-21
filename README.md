# edet

edet is a decentralized mutual credit protocol built on Holochain. Instead of transferring tokens, buying creates a debt obligation: when you buy something, you owe the equivalent value to your seller, which you repay by selling goods or services to anyone in your network. Debt propagates forward through the economy rather than money flowing backward.

## Why edet is different

**vs. traditional mutual credit (Circles, Trustlines, Sardex)**
Existing systems require bilateral credit limits negotiated in advance between
every pair of trading partners. edet replaces bilateral limits with a
network-wide EigenTrust reputation score. Your credit capacity is derived from
what the network collectively thinks of you, not from individual handshake
agreements — you can trade with strangers without prior negotiation.

**vs. token-based DeFi (DAI, USDC, algorithmic stablecoins)**
Tokens have a speculative premium: holders accumulate them hoping they
appreciate, which creates hoarding and deflationary pressure. edet has no
token to hoard. Debt obligations expire (30-day maturity), so the only
rational strategy is to participate — buy, sell, repay. The medium of exchange
has no speculative value because it exists only as an obligation that must be
discharged.

**vs. reputation-gated lending (TrustCredo, Karma)**
Most reputation lending systems have a central authority computing
creditworthiness. edet is fully subjective: every agent runs their own
EigenTrust computation over their local acquaintance subgraph. There is no
global credit score, no central oracle, no issuer. Two observers can disagree
on your capacity — that is by design.

**vs. Holochain mutual credit (HoloFuel, RedGrid)**
HoloFuel is pure accounting with no reputation-gated capacity. edet adds a
formal reputation layer — EigenTrust power iteration, satisfaction/failure
counters, trust attenuation, and contagion propagation — that bounds how much
debt a newcomer or bad actor can accumulate before the network excludes them.

## Key properties

- **Spend what you earn, not what you buy** — edet doesn't use tokens or coins. Your ability to buy things is a direct reflection of your reputation as an honest trader.
- **Buying creates a debt, selling pays it back** — When you buy something, you don't "lose" money; you gain a debt. You clear this debt by selling your own services or products to others in the network.
- **Trust flows through your network** — There is no central bank or credit bureau. Your spending limit is determined by the trust your local neighbors have in you, which spreads through the network like a web.
- **Start small, grow fast** — New accounts start with a small, safe "trial" limit. As you complete trades and follow through on your obligations, the network automatically increases your capacity to match your activity.
- **Proven history, not fake accounts** — Creating thousands of fake accounts doesn't help an attacker. The protocol requires newcomers to prove themselves through small, successful transactions before they can access significant credit.
- **No fees, no middlemen** — Transactions are approved by you and your peers directly. There are no gas fees, no miners, and no central authority taking a cut.

## Repository layout

```
dnas/edet/
  zomes/
    integrity/transaction/   # Entry types, link types, validation rules
    coordinator/transaction/  # EigenTrust, risk score, cascade, capacity
doc/
  edet.pdf                    # Whitepaper with formal proofs
  edet.tex                    # LaTeX source
sim/                          # Python simulation + theorem verification
  main.py                     # Run a full N-agent simulation
  verify_theory.py            # Verify all 13 security theorems
  config.py                   # Protocol parameters (aligned with constants.rs)
tests/sweettest/              # Rust integration tests (134 tests)
ui/                           # Svelte frontend
workdir/                      # Packaged .happ and .webhapp outputs
```

## Getting started

> **Prerequisite:** set up the
> [Holochain development environment](https://developer.holochain.org/docs/install/).

Enter the nix shell from the repository root:

```bash
nix develop
npm install
```

All commands below must be run inside this nix shell.

## Running locally

Start a 2-agent network with UI and Holochain Playground:

```bash
npm run start
```

To run more agents:

```bash
AGENTS=5 npm run network
```

## Running tests

```bash
npm run test
```

Builds the test DNA (integrity zome compiled with `test-epoch` feature for
accelerated epoch timing) then runs all 134 integration tests via
`cargo nextest`. Expected output: `134 tests run: 134 passed`.

## Running simulations

```bash
cd sim
python3 main.py          # simulate N=1000 agents, 100 epochs
python3 verify_theory.py # verify all 13 whitepaper security theorems
```

Simulation parameters are in `sim/config.py` and are kept in sync with the
protocol constants in
`dnas/edet/zomes/integrity/transaction/src/types/constants.rs`.

## Building and packaging

Build the production hApp:

```bash
npm run build:happ
```

Package as a distributable `.webhapp` for the Holochain Launcher:

```bash
npm run package
```

Output is written to `workdir/edet.webhapp`.

## Documentation

The formal whitepaper (`doc/edet.pdf`) covers:

- Protocol definition and debt lifecycle (Sections 1–3)
- EigenTrust adaptation, convergence proof, and subgraph approximation (Section 4)
- Credit capacity formula and two-layer enforcement (Section 5)
- Security analysis: 13 theorems covering Sybil isolation, flash loans,
  whitewashing, eclipse attacks, gateway attacks, and more (Section 6)
- Implementation notes and parameter derivations (Appendix B)

## Tooling

| Tool | Purpose |
|------|---------|
| [hc](https://github.com/holochain/holochain/tree/develop/crates/hc) | Holochain CLI — pack DNAs, manage sandboxes |
| [cargo nextest](https://nexte.st) | Fast Rust test runner |
| [@holochain/client](https://www.npmjs.com/package/@holochain/client) | UI ↔ conductor WebSocket client |
| [@holochain-playground/cli](https://www.npmjs.com/package/@holochain-playground/cli) | DHT introspection during development |