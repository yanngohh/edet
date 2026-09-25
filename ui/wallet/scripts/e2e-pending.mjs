// Two-device end-user simulation, headless: device A (member 0, node :7501)
// records a purchase; the transaction parks in the pending pool; device B
// (member 1, node :7502) finds it awaiting signature, co-signs from ITS
// node; the contract commits and both nodes agree. Exactly the flow two
// browser windows on different ports exercise interactively.
//
//   node scripts/e2e-pending.mjs [baseA] [baseB]

import * as ed from '@noble/ed25519';
import { sha512 } from '@noble/hashes/sha2';

import { entropyToMnemonic, mnemonicToSeedSync } from '@scure/bip39';
import { wordlist } from '@scure/bip39/wordlists/english';

ed.etc.sha512Sync = (...m) => sha512(ed.etc.concatBytes(...m));

// Founder seeds derive from the published dev recovery phrases
// (entropy [id+1; 16]) — the same derivation the app's restore flow uses.
const devSeed = (id) =>
  mnemonicToSeedSync(entropyToMnemonic(new Uint8Array(16).fill((id + 1) & 0xff), wordlist)).slice(0, 32);

const baseA = process.argv[2] ?? 'http://127.0.0.1:7501';
const baseB = process.argv[3] ?? 'http://127.0.0.1:7502';

const hexToBytes = (hex) => Uint8Array.from(hex.match(/.{2}/g).map((b) => parseInt(b, 16)));

async function post(base, path, body) {
  const res = await fetch(`${base}${path}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!res.ok) throw new Error(`${path}: ${res.status}`);
  return res.json();
}
const getJson = async (base, path) => (await fetch(`${base}${path}`)).json();
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const tx = { Accept: { debtor: 0, creditor: 1, amount: 25.0, maturity_epochs: 30, arb: null } };

// Device A: sign own side only, park in the pool.
const { digest } = await post(baseA, '/tx/digest', tx);
const signAs = (id) => ({
  tx,
  required: [0, 1],
  min_sigs: 2,
  signer: Array.from(ed.getPublicKey(devSeed(id))),
  signature: Array.from(ed.sign(hexToBytes(digest), devSeed(id))),
});
const a = await post(baseA, '/pending/sign', signAs(0));
if (!a.ok || a.completed) throw new Error(`unexpected propose result: ${JSON.stringify(a)}`);

// Device B: the request must appear on ITS node via gossip.
let awaiting = null;
for (let i = 0; i < 40 && !awaiting; i++) {
  const p = await getJson(baseB, '/pending/1');
  if (p.awaiting_me?.length === 1) awaiting = p.awaiting_me[0];
  else await sleep(100);
}
if (!awaiting) throw new Error('request never reached device B');
if (awaiting.digest !== digest) throw new Error('digest mismatch across nodes');

// Device B co-signs on its own node → threshold met → committed.
const b = await post(baseB, '/pending/sign', signAs(1));
if (!b.ok || !b.completed || !b.queued) throw new Error(`co-sign did not complete: ${JSON.stringify(b)}`);

for (let i = 0; i < 30; i++) {
  await sleep(200);
  const [ca, cb] = await Promise.all([getJson(baseA, '/contracts'), getJson(baseB, '/contracts')]);
  if (ca.length === 1 && cb.length === 1) {
    const [na, nb] = await Promise.all([getJson(baseA, '/network'), getJson(baseB, '/network')]);
    if (na.state_hash === nb.state_hash) {
      console.log('ok: A proposed, B co-signed from its own node, contract committed, hashes agree');
      process.exit(0);
    }
  }
}
throw new Error('co-signed contract did not commit on both nodes');
