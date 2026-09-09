// E2E: the first-contact journey, walked as an ORDINARY member.
//
// This gate exists because two separate breaks reached the branch, both on
// the paths a new member meets first, and neither was visible to any existing
// gate:
//
//   1. Onboarding could not complete on a fresh device. `/whois` is how a
//      restoring device learns its own member id, but every credential the
//      node accepted required that id already.
//   2. A member who was not a validator could not resolve a counterparty's
//      address, so `AddressInput` reported "unknown" and no purchase could be
//      entered at all.
//
// Both were masked the same way: `dev_genesis` seeds every founder as a
// validator, and every harness only ever acted as a founder. So the one thing
// this script must never do is act as a founder. It creates a real ordinary
// account and does everything as it.
//
// **There is no admission, and there is no creation either.** The
// journey this walks is the v0.7.0 one: an established member takes a first
// UNINSURED risk on a key nobody knows, and THAT TRADE is what puts the account
// on the ledger. It is worth exactly zero the moment it exists, the trade
// settles, and capacity is the residue. Nothing is approved anywhere along it,
// which is the property most worth walking against a real node.
//
// The step this replaces was "a key signs its own account into existence"
// (`OpenAccount`). That transition was unbillable — a key nobody knows has no
// headroom — so it could only be bounded by a ledger-wide per-epoch counter,
// which is a censorship lever rather than a quota: measured, an attacker with
// no standing filled the whole 1024 for a bond spend of zero and shut
// onboarding for everybody until the boundary. The row is now seated by the
// bonded trade that names it, so the write is priced the same for everybody
// and the counter is gone.
//
//   node scripts/e2e-first-contact.mjs [baseUrl]
//
// Exits 0 when the whole journey completes, 1 on the first broken step.

import { webcrypto } from 'node:crypto';
import * as ed from '@noble/ed25519';
import { sha512 } from '@noble/hashes/sha2';
import { entropyToMnemonic, mnemonicToSeedSync } from '@scure/bip39';
import { wordlist } from '@scure/bip39/wordlists/english';

ed.etc.sha512Sync = (...m) => sha512(ed.etc.concatBytes(...m));

const base = process.argv[2] ?? 'http://127.0.0.1:7401';
const hex = (b) => Buffer.from(b).toString('hex');
const hexToBytes = (h) => Uint8Array.from(h.match(/.{2}/g).map((b) => parseInt(b, 16)));
const devSeed = (id) =>
    mnemonicToSeedSync(entropyToMnemonic(new Uint8Array(16).fill((id + 1) & 0xff), wordlist)).slice(0, 32);

let failures = 0;
function check(ok, label, detail = '') {
    console.log(`  ${ok ? 'ok  ' : 'FAIL'}  ${label}${detail ? ` — ${detail}` : ''}`);
    if (!ok) failures++;
    return ok;
}

async function get(path, headers = {}) {
    const res = await fetch(`${base}${path}`, { headers });
    return { status: res.status, body: await res.json().catch(() => null) };
}
async function post(path, body, headers = {}) {
    const res = await fetch(`${base}${path}`, {
        method: 'POST',
        headers: { 'content-type': 'application/json', ...headers },
        body: JSON.stringify(body),
    });
    if (!res.ok) throw new Error(`${path}: ${res.status}`);
    return res.json();
}

// `/tx/check` discloses its VERDICT only to a party to the transaction — the
// reply code is a probe of the debtor's headroom otherwise. A real wallet
// pre-flights its own draft while signed in; so does this.
async function checkTx(envelope, seed) {
    return post('/tx/check', envelope, await keyProofHeaders(seed, '/tx/check', 'POST'));
}

/** The chain every credential and every envelope here binds to, read once. */
let CHAIN = null;
async function chainId() {
    if (CHAIN === null) {
        CHAIN = (await get('/network')).body?.chain_id ?? '';
    }
    return CHAIN;
}

/** The key-addressed viewer proof, byte-identical to `lib/session.ts::keyProof`. */
async function keyProofHeaders(seed, path, method = 'GET') {
    const ts = Math.floor(Date.now() / 1000);
    // A fresh 16-byte nonce per credential. The node admits each verified
    // signature once inside its window, and Ed25519 is deterministic — so two
    // reads of one path in one second would otherwise be one credential and
    // the second would come back 401.
    const nonce = hex(webcrypto.getRandomValues(new Uint8Array(16)));
    // Byte-identical to `crates/node/src/serve/auth.rs::viewer_auth_message`:
    // a domain tag, then the chain this credential authenticates against, then
    // the request, its timestamp and its nonce.
    const msg = new TextEncoder().encode(`edet-view-v2\n${await chainId()}\n${method} ${path}\n${ts}\n${nonce}`);
    return {
        'x-edet-viewer-key': hex(await ed.getPublicKeyAsync(seed)),
        'x-edet-viewer-sig': hex(await ed.signAsync(msg, seed)),
        'x-edet-viewer-ts': String(ts),
        'x-edet-viewer-nonce': nonce,
    };
}
const whoisAs = async (needle, seed) =>
    (await get(`/whois/${needle}`, await keyProofHeaders(seed, `/whois/${needle}`))).body;
// Public keys are not in the visibility matrix's public row, so a caller that
// wants to find a member BY key must authenticate. The sponsor does this to
// watch its own admission land.
const membersAs = async (seed) => (await get('/members', await keyProofHeaders(seed, '/members'))).body;

/**
 * The epoch the node's envelope check compares against, found by bisection.
 * `/network` reports the COMMITTED epoch, which is not it: a dry-run runs
 * `begin_block` first, and on a wall-clock node that jumps the epoch forward
 * (capped) before the window is checked. ET-TX-002 means "below the epoch",
 * ET-TX-003 means "too far above it", so the boundary between them IS the
 * epoch — no need to re-derive the cap chain here, and this works unchanged
 * on a logical-clock node.
 */
async function effectiveEpoch() {
    const probe = async (n) =>
        (
            await checkTx(
                {
                    tx: { MarkExpired: { contract: 0 } },
                    nonce: new Array(16).fill(1),
                    not_after_epoch: n,
                    signers: [Array.from(await ed.getPublicKeyAsync(devSeed(0)))],
                    signatures: [],
                },
                devSeed(0),
            )
        ).code;
    let lo = 0;
    let hi = 1_000_000;
    while (lo < hi) {
        const mid = Math.floor((lo + hi) / 2);
        if ((await probe(mid)) === 'ET-TX-002') lo = mid + 1;
        else hi = mid;
    }
    return lo;
}

/**
 * Sign and submit, then report whether it was queued.
 *
 * `preflight` is off for the create path, and that is a property rather than a
 * convenience. `/tx/check` discloses its verdict only to a party — a viewer
 * whose key resolves to a member among the signers, or a validator — because
 * the reply code is otherwise a one-bit probe of any named member's private
 * headroom. A key that has not yet opened an account resolves to no member at
 * all, so it gets the public half (the bond quote) and nothing else. A real
 * wallet is in exactly that position on first launch: it cannot pre-flight,
 * because there is nobody to pre-flight as.
 */
async function submit(tx, seeds, label, preflight = true) {
    const nonce = Array.from(crypto.getRandomValues(new Uint8Array(16)));
    const not_after_epoch = (await effectiveEpoch()) + 5;
    const { digest } = await post('/tx/digest', { tx, nonce, not_after_epoch });
    const signers = [];
    const signatures = [];
    for (const s of seeds) {
        signers.push(Array.from(await ed.getPublicKeyAsync(s)));
        signatures.push(Array.from(await ed.signAsync(hexToBytes(digest), s)));
    }
    const envelope = { tx, nonce, not_after_epoch, signers, signatures };
    if (preflight) {
        const chk = await checkTx(envelope, seeds[0]);
        if (!chk.ok) return { ok: false, code: chk.code ?? 'no verdict for a non-party', label };
    }
    const sub = await post('/tx', envelope);
    if (!sub.queued) return { ok: false, code: 'not-queued', label };
    return { ok: true, label };
}

async function waitFor(predicate, tries = 40, ms = 250) {
    for (let i = 0; i < tries; i++) {
        if (await predicate()) return true;
        await new Promise((r) => setTimeout(r, ms));
    }
    return false;
}

// ---------------------------------------------------------------------------
console.log(`first-contact journey against ${base}\n`);

// --- 0. A brand-new key, unknown to the ledger -----------------------------
console.log('a newly created identity, before it has signed anything:');
const newcomer = mnemonicToSeedSync(
    entropyToMnemonic(new Uint8Array(16).fill(0xab), wordlist),
).slice(0, 32);
const newcomerKey = await ed.getPublicKeyAsync(newcomer);
const newcomerKeyHex = hex(newcomerKey);

const members0 = await membersAs(devSeed(0));
const preexisting = members0.find((m) => m.keys?.includes(newcomerKeyHex));
if (!preexisting) {
    const pre = await whoisAs(newcomerKeyHex, newcomer);
    // A sound proof naming a key the ledger does not know must read as "not
    // yet", never as an error — otherwise the create-path poll cannot run.
    check(pre && pre.member === null && !pre.error, 'an unknown key polls as {"member": null}', JSON.stringify(pre));
}

// --- 1. The first trade is what seats the account --------------------------
//
// The newcomer is named by KEY, because they have no member id to be named by
// — that is the whole of what naming a key says. Both sides sign, as `Accept`
// has always required, so the key-control proof `OpenAccount` used to take
// arrives through the signature the trade already needed. The bond lands on
// the founder, who is the party that chose to take the risk.
//
// The two signatures are assembled OFF the node and submitted as one envelope,
// which is also the wallet's shape: a key cannot open a pending-pool entry
// (the node test `a_key_with_no_account_cannot_open_a_pending_proposal` gates that), so the
// newcomer signs their purchase on their own device and hands it to the seller
// as a code (`ui/src/lib/offer.ts`), and the seller's device co-signs and
// submits exactly this.
//
// It is deliberately the SAME transaction as step 4's first trade: there is no
// separate creation step to walk any more, which is the change itself.
const net0 = (await get('/network')).body;
const amount = 400;
let newcomerId = preexisting?.id;
if (newcomerId === undefined) {
    const seating = await submit(
        {
            Accept: {
                debtor: { Key: Array.from(newcomerKey) },
                creditor: { Member: 0 },
                amount,
                maturity_epochs: net0.min_maturity_epochs ?? 30,
                arb: null,
            },
        },
        [newcomer, devSeed(0)],
        'first trade',
        false,
    );
    if (!check(seating.ok, `an established member takes a first risk of ${amount} on a key nobody knows`, seating.code ?? '')) {
        process.exit(1);
    }
    const landed = await waitFor(async () =>
        (await membersAs(devSeed(0))).find((m) => m.keys?.includes(newcomerKeyHex)),
    );
    if (!check(landed, 'and the account it seats commits')) process.exit(1);
    // `waitFor` answers whether the predicate ever held, not with what it
    // found, so the row is re-read once it has.
    newcomerId = (await membersAs(devSeed(0))).find((m) => m.keys?.includes(newcomerKeyHex)).id;
}

const detail = (await get(`/member/${newcomerId}`, await keyProofHeaders(newcomer, `/member/${newcomerId}`))).body;
// If this ever becomes true the whole script stops proving anything: every
// break it guards was invisible precisely because the actor was a validator.
if (!check(detail.is_validator === false, `member ${newcomerId} is NOT a validator`, 'this is what makes the run meaningful')) {
    process.exit(1);
}
check(detail.capacity === 0, 'and is worth exactly zero, by arithmetic rather than by rule', JSON.stringify(detail.capacity));

// The transition that used to seat it is gone, and a node that still accepted
// one would be running a different alphabet. Checked at the WIRE, because a
// retired transition comes back through a client or a stored transaction.
const retired = await post('/tx/digest', { tx: { OpenAccount: { keys: [Array.from(newcomerKey)] } }, nonce: new Array(16).fill(3), not_after_epoch: 30 }).catch(
    (e) => ({ error: String(e) }),
);
check(!retired.digest, 'and the account-creation transition no longer even encodes', JSON.stringify(retired));

// --- 2. Onboarding: the device learns its own id from its key ---------------
console.log('\nonboarding, as the newcomer:');
const self = await whoisAs(newcomerKeyHex, newcomer);
check(self?.member === newcomerId, 'restore-from-phrase resolves this device to its member id', JSON.stringify(self));

const anon = (await get(`/whois/${newcomerKeyHex}`)).body;
check(!!anon?.error, 'an anonymous scan of that same key is still refused', JSON.stringify(anon));

// --- 3. Finding someone to trade with --------------------------------------
console.log('\nfinding a counterparty, as the newcomer:');
const members = await membersAs(newcomer);
const founder = members.find((m) => m.id === 0);
const founderAddr = founder.address.replace(/^0x/, '');
const resolved = await whoisAs(founderAddr, newcomer);
check(
    resolved?.member === founder.id,
    "an ordinary member resolves a counterparty's address (AddressInput)",
    JSON.stringify(resolved),
);

// --- 4. The first trade is uninsured, and it is what creates standing -------
//
// The whole bootstrap, against a real node. Nobody has backed the newcomer, so
// there is no flow to reserve and the obligation falls to the uninsured tier —
// the founder bears it alone, which is the real cost §9 names and the reason
// no mechanism can supply the willingness. It settles, the settlement writes a
// stake, and capacity is its residue.
console.log('\ntransacting, as the newcomer:');
// The obligation booked by the trade that seated them, read back as themselves
// — now by member id, which they have because that trade committed.
const mine = (await get(`/member/${newcomerId}`, await keyProofHeaders(newcomer, `/member/${newcomerId}`))).body;
const opened = mine.owes.find((c) => c.outstanding === amount) ?? mine.owes[0];
if (!check(!!opened, 'the first trade left an obligation on the newcomer', JSON.stringify(mine.owes))) process.exit(1);
check(opened?.insured === false, 'and it is UNINSURED — nothing backs the newcomer yet', JSON.stringify(opened?.insured));

// Now that they ARE a member, naming them by id is the ordinary path, and it
// seats nothing: the row exists.
const beforeSecond = (await get('/contracts')).body.length;
const second = await submit(
    {
        Accept: {
            debtor: { Member: newcomerId },
            creditor: { Member: founder.id },
            amount: 10,
            maturity_epochs: net0.min_maturity_epochs ?? 30,
            arb: null,
        },
    },
    [newcomer, devSeed(0)],
    'second purchase',
);
if (!check(second.ok, 'a member trades by id, with no seating involved', second.code ?? '')) process.exit(1);
check(
    await waitFor(async () => (await get('/contracts')).body.length > beforeSecond),
    'and that obligation commits too',
);

// --- 5. Settling it is what confers standing --------------------------------
const settled = await submit({ Settle: { contract: opened.id, amount } }, [newcomer, devSeed(0)], 'settle');
if (!check(settled.ok, 'the newcomer honours it', settled.code ?? '')) process.exit(1);
const conferred = await waitFor(async () => {
    const m = (await get(`/member/${newcomerId}`, await keyProofHeaders(newcomer, `/member/${newcomerId}`))).body;
    return m.capacity > 0;
});
check(conferred, 'and the settlement confers capacity — the residue of trade, never its precondition');

console.log(
    failures === 0
        ? '\nPASS: a trade seats an ordinary account, which then onboards, finds a counterparty, trades, and earns standing.'
        : `\nFAIL: ${failures} step(s) broken.`,
);
process.exit(failures === 0 ? 0 : 1);
