// E2E signature smoke: sign a transaction with the primitive the browser
// client uses (noble ed25519) over a digest fetched from the node this harness
// started — a wallet computes its own (`src/lib/txdigest.ts`); this script
// holds no member's seed — and submit it to a node with signature verification
// ON. Proves noble ↔ dalek agreement end to end, and pins the envelope digest
// (chain id + nonce + validity window) against a golden vector taken from the
// Rust reference implementation.
//
//   node scripts/e2e-sign.mjs [baseUrl]
//
// Exits 0 when the golden digest matches, the signed tx queues AND commits,
// and a forged one is refused.

import * as ed from '@noble/ed25519';
import { sha512 } from '@noble/hashes/sha2';

import { entropyToMnemonic, mnemonicToSeedSync } from '@scure/bip39';
import { wordlist } from '@scure/bip39/wordlists/english';

ed.etc.sha512Sync = (...m) => sha512(ed.etc.concatBytes(...m));

// Founder seeds derive from the published dev recovery phrases
// (entropy [id+1; 16]) — the same derivation the app's restore flow uses.
const devSeed = (id) =>
  mnemonicToSeedSync(entropyToMnemonic(new Uint8Array(16).fill((id + 1) & 0xff), wordlist)).slice(0, 32);

const base = process.argv[2] ?? 'http://127.0.0.1:7501';

const hexToBytes = (hex) => Uint8Array.from(hex.match(/.{2}/g).map((b) => parseInt(b, 16)));

async function post(path, body) {
  const res = await fetch(`${base}${path}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!res.ok) throw new Error(`${path}: ${res.status}`);
  return res.json();
}

const tx = { Accept: { debtor: 0, creditor: 1, amount: 25.0, maturity_epochs: 30, arb: null } };

// Every envelope needs a fresh, unpredictable nonce (CSPRNG — never
// Math.random, exactly like `ui/src/lib/crypto.ts::randomNonce`) and a
// validity window. The window is read off the node's OWN current epoch,
// never a hardcoded absolute one — mirrors `ui/src/lib/submit.ts::
// defaultNotAfterEpoch` exactly, and for the same reason: this binary runs
// on wall-clock time (`Config::wall_clock`), so its epoch is already far
// past 0 by the time this script connects, and a small fixed window would
// be expired (ET-TX-002) before it's ever submitted.
//
// NOTE on the window's value: a wall-clock node's `begin_block` (`state.rs`)
// advances the epoch to `min(now_secs / epoch_secs, self.epoch +
// MAX_EPOCH_ADVANCE_PER_BLOCK)` (the structural cap on epoch-closing work),
// and the cap is RELATIVE TO WHATEVER `self.epoch` ALREADY IS at the moment
// of the call — not to the last COMMITTED epoch. That matters here because
// `begin_block` is currently invoked TWICE for a one-transaction block on
// both the path this script exercises and the real commit path:
// `check_tx`'s dry run (`serve/views.rs::check_tx`) calls it explicitly
// before handing off to `edet_state::apply`, which calls it AGAIN
// internally (`state/src/apply.rs`, first line) — and `Replica::
// apply_block_to` (`replica.rs`) has the identical shape on commit. Two
// calls from a fresh genesis (committed epoch 0) each apply the cap
// relative to their own starting point, so the epoch lands at
// `min(now/epoch_secs, 2 * MAX_EPOCH_ADVANCE_PER_BLOCK)`, not
// `MAX_EPOCH_ADVANCE_PER_BLOCK` — this script predicts the ACTUAL landing
// spot, not the single-call formula `lib/submit.ts::defaultNotAfterEpoch`
// documents (that one is correct for its own case: a long-lived cluster
// whose committed epoch already tracks real time, where a second call is a
// no-op because the first already reached `now`). If `check_tx`'s redundant
// call is ever removed, this reverts to a single cap and the multiplier
// below drops from 2 to 1 — a stale prediction fails LOUDLY (a rejected
// dry-run) rather than silently, so drift here cannot hide a real envelope
// bug.
const randomNonce = () => Array.from(crypto.getRandomValues(new Uint8Array(16)));
const nonce = randomNonce();
const EPOCH_SECS = 86_400; // edet_kernel::constants::EPOCH_SECS
const MAX_EPOCH_ADVANCE_PER_BLOCK = 10_000; // edet_kernel::constants::MAX_EPOCH_ADVANCE_PER_BLOCK
const { epoch: committedEpoch } = await (await fetch(`${base}/network`)).json();
const realEpoch = Math.floor(Date.now() / 1000 / EPOCH_SECS);
const landingEpoch = Math.min(realEpoch, committedEpoch + 2 * MAX_EPOCH_ADVANCE_PER_BLOCK);
const notAfterEpoch = landingEpoch + 15; // mid-window margin, well clear of both bounds

// 0. Golden-vector cross-pin: the SAME fixed (chain_id, tx, nonce,
// window) must always digest to the SAME bytes. GOLDEN_DIGEST was computed
// ONCE straight from the normative Rust implementation
// (`crates/node/src/block.rs::tx_digest`) against exactly this vector — see
// that function's doc comment for the exact byte layout it hashes. This
// client never reimplements that encoding (bincode's enum/struct layout is
// not worth reproducing in JS — the client always asks the node for the
// digest it is about to sign, see `ui/src/lib/crypto.ts`'s doc comment), so
// the request below still goes over the wire; what's pinned is the NODE's
// own answer for a fixed input, catching a drift in `tx_digest`'s byte
// layout (a field reordering, a domain-tag change, ...) the moment this
// script next runs, rather than as a signature the node silently rejects.
// Assumes the target node runs the dev/testnet genesis (`DEV_CHAIN_ID`,
// "edet-dev") — true for every harness this script is documented to run
// against.
const GOLDEN_TX = { Accept: { debtor: 0, creditor: 1, amount: 25.0, maturity_epochs: 30, arb: null } };
const GOLDEN_NONCE = new Array(16).fill(0);
const GOLDEN_NOT_AFTER_EPOCH = 30;
const GOLDEN_DIGEST = '92c705f795bd78601534a8c5aa75a1b327faa9d6adf86994f2250ab1a8a62862';
{
  const { digest } = await post('/tx/digest', { tx: GOLDEN_TX, nonce: GOLDEN_NONCE, not_after_epoch: GOLDEN_NOT_AFTER_EPOCH });
  if (digest !== GOLDEN_DIGEST) {
    throw new Error(`tx_digest drifted from the pinned golden vector: got ${digest}, want ${GOLDEN_DIGEST}`);
  }
}

// 1. Digest from the node, signatures from noble — the UI's exact write path.
const { digest } = await post('/tx/digest', { tx, nonce, not_after_epoch: notAfterEpoch });
const signers = [];
const signatures = [];
for (const id of [0, 1]) {
  const seed = devSeed(id);
  signers.push(Array.from(ed.getPublicKey(seed)));
  signatures.push(Array.from(ed.sign(hexToBytes(digest), seed)));
}

const envelope = { tx, nonce, not_after_epoch: notAfterEpoch, signers, signatures };

const check = await post('/tx/check', envelope);
if (!check.ok) throw new Error(`check rejected: ${check.code}`);

const good = await post('/tx', envelope);
if (!good.queued) throw new Error('properly signed tx was NOT queued — noble/dalek mismatch?');

// 2. A forged submission (signature bytes flipped) must be refused.
const forged = signatures.map((s) => { const c = s.slice(); c[0] ^= 0xff; return c; });
const bad = await post('/tx', { ...envelope, signatures: forged });
if (bad.queued) throw new Error('FORGED tx was accepted!');

// 3. Wait for the commit.
for (let i = 0; i < 20; i++) {
  await new Promise((r) => setTimeout(r, 300));
  const contracts = await (await fetch(`${base}/contracts`)).json();
  if (contracts.length > 0) {
    console.log('ok: signed tx committed as contract', contracts[0].id, '| forged tx refused');
    process.exit(0);
  }
}
throw new Error('signed tx queued but never committed');
