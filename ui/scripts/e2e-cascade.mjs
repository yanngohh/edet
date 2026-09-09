// Cascade discharge, end to end: "you just buy — and get discharged when
// you sell or are supported." Proves on a live node that
//   1. selling clears your own debts first: the buyer assumes them toward
//      your original creditors (the automatic transfer), and
//   2. a supporter's sale drains into YOUR debts the same way, and
//   3. when your CREDITOR buys from you, the mutual debt nets inside the
//      sale (settles, or cures if expired),
// with no manual Settle/Transfer/Cure anywhere.
//
//   node scripts/e2e-cascade.mjs [baseUrl]

import * as ed from '@noble/ed25519';
import { sha512 } from '@noble/hashes/sha2';
import { entropyToMnemonic, mnemonicToSeedSync } from '@scure/bip39';
import { wordlist } from '@scure/bip39/wordlists/english';

ed.etc.sha512Sync = (...m) => sha512(ed.etc.concatBytes(...m));

const devSeed = (id) =>
  mnemonicToSeedSync(entropyToMnemonic(new Uint8Array(16).fill((id + 1) & 0xff), wordlist)).slice(0, 32);

const base = process.argv[2] ?? 'http://127.0.0.1:7501';
const hexToBytes = (hex) => Uint8Array.from(hex.match(/.{2}/g).map((b) => parseInt(b, 16)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function post(path, body) {
  const res = await fetch(`${base}${path}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!res.ok) throw new Error(`${path}: ${res.status}`);
  return res.json();
}
const getJson = async (path) => (await fetch(`${base}${path}`)).json();

async function submit(tx, signerIds) {
  const { digest } = await post('/tx/digest', tx);
  const bytes = hexToBytes(digest);
  const signers = signerIds.map((id) => Array.from(ed.getPublicKey(devSeed(id))));
  const signatures = signerIds.map((id) => Array.from(ed.sign(bytes, devSeed(id))));
  const r = await post('/tx', { tx, signers, signatures });
  if (!r.queued) throw new Error(`not queued: ${JSON.stringify(tx)}`);
}

async function waitFor(pred, what, tries = 60) {
  for (let i = 0; i < tries; i++) {
    const contracts = await getJson('/contracts');
    const hit = pred(contracts);
    if (hit) return hit;
    await sleep(200);
  }
  throw new Error(`timeout waiting for ${what}`);
}

// --- bootstrap standing (mirrors the node demo: 6 rounds of trade) --------
// Amounts vary per round: byte-identical transactions are refused as
// replays by the mempool's seen-set.
for (let round = 0; round < 6; round++) {
  for (let w = 0; w < 5; w++) {
    await submit(
      { Accept: { debtor: w, creditor: (w + 1) % 5, amount: 40.0 + round, maturity_epochs: 30, arb: null } },
      [w, (w + 1) % 5],
    );
  }
  await waitFor((cs) => (cs.filter((c) => c.status === 'active').length === 5 ? true : null), 'round accepts');
  const active = (await getJson('/contracts')).filter((c) => c.status === 'active');
  for (const c of active) {
    await submit({ Settle: { contract: c.id, amount: c.outstanding } }, [c.debtor, c.creditor]);
  }
  await waitFor((cs) => (cs.every((c) => c.status !== 'active') ? true : null), 'round settles');
}

// --- 1. self-discharge: 0 owes 1; 0 sells to 2; the debt moves to 2 -------
await submit({ Accept: { debtor: 0, creditor: 1, amount: 30.0, maturity_epochs: 30, arb: null } }, [0, 1]);
const debt01 = await waitFor(
  (cs) => cs.find((c) => c.status === 'active' && c.debtor === 0 && c.creditor === 1) ?? null,
  'debt 0→1',
);

await submit({ Sale: { seller: 0, buyer: 2, amount: 30.0, maturity_epochs: 30 } }, [0, 2]);
await waitFor(
  (cs) => (cs.find((c) => c.id === debt01.id)?.status === 'transferred' ? true : null),
  'debt 0→1 auto-transferred by the sale',
);
const assumed21 = await waitFor(
  (cs) => cs.find((c) => c.status === 'active' && c.debtor === 2 && c.creditor === 1 && Math.abs(c.outstanding - 30) < 0.01) ?? null,
  'buyer 2 assuming the debt toward creditor 1',
);
const m0 = await getJson('/member/0');
if (m0.debt > 0.01) throw new Error(`seller 0 still owes ${m0.debt} after selling`);

// --- 2. supported: 3 owes 4; 0 lists+is approved by 3; 0's sale drains it --
await submit({ Accept: { debtor: 3, creditor: 4, amount: 20.0, maturity_epochs: 30, arb: null } }, [3, 4]);
await waitFor(
  (cs) => cs.find((c) => c.status === 'active' && c.debtor === 3 && c.creditor === 4) ?? null,
  'debt 3→4',
);
await submit({ ListBeneficiaries: { supporter: 0, entries: [[3, 1.0]] } }, [0]);
await submit({ ApproveSupporter: { beneficiary: 3, supporter: 0, approved: true } }, [3]);
await sleep(600);

await submit({ Sale: { seller: 0, buyer: 2, amount: 20.0, maturity_epochs: 30 } }, [0, 2]);
await waitFor(
  (cs) => cs.find((c) => c.status === 'active' && c.debtor === 2 && c.creditor === 4 && Math.abs(c.outstanding - 20) < 0.01) ?? null,
  "supporter's sale draining into 3's debt (buyer 2 assumes toward 4)",
);
const m3 = await getJson('/member/3');
if (m3.debt > 0.01) throw new Error(`beneficiary 3 still owes ${m3.debt} after being supported`);

// --- 3. netting: creditor 1 buys from debtor 2; the mutual debt settles ----
// 2 now owes 1 (30, assumed above) and 4 (20). Buying from your debtor
// extinguishes the mutual contract inside the sale itself — no Settle tx.
await submit({ Sale: { seller: 2, buyer: 1, amount: 30.0, maturity_epochs: 30 } }, [2, 1]);
await waitFor(
  (cs) => (cs.find((c) => c.id === assumed21.id)?.status === 'settled' ? true : null),
  'mutual debt 2→1 settled by the sale (netting)',
);
const m2 = await getJson('/member/2');
if (Math.abs(m2.debt - 20) > 0.01) throw new Error(`debtor 2 owes ${m2.debt}, expected 20 (only the 2→4 leg)`);
const m1 = await getJson('/member/1');
if (m1.debt > 0.01) throw new Error(`buyer 1 assumed ${m1.debt} — netting must create no new debt`);

console.log('ok: sale auto-cleared own debt (assumed by buyer) + supporter sale auto-cleared beneficiary debt + creditor purchase netted the mutual debt — no manual settle/transfer/cure');
process.exit(0);
