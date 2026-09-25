/**
 * The node connection: one store that polls the selected network's node and
 * exposes every view the sections render from.
 *
 * One instance, one network, one identity — like production. The network is
 * chosen by the member (`lib/networks.ts`), or pinned by the launcher via
 * `EDET_NODE` (`just dev` binds each UI instance to its own node). There is
 * still no in-app IDENTITY switching: to be another user, run another
 * instance, whose isolated origin keeps its vault apart.
 *
 * The app embeds no node and reads none over IPC, which is why this
 * file had an `inTauri` branch in every binding. It is a client of a node it
 * does not run now, on every host.
 */

import { derived, get, writable } from 'svelte/store';

import * as api from './api';
import { adoptPendingSeed, chooseActor, currentActorId, heldSeed, pendingIdentity } from './actors';
import { clearSession, currentToken, ensureSession, keyProof, remintSession, viewerChain } from './session';
import { bytesToHex, derivePublicKey } from './crypto';
import { lsGet, lsSet } from '../common/safeStorage';
import { DEFAULT_NETWORK, declaredChainId, resolveNodes } from './networks';

/** Node URL injected by the launcher (per-instance binding). */
const envNode: string | undefined = (import.meta as any).env?.EDET_NODE || undefined;

/** True when the node binding comes from the launcher and is not editable. */
export const nodeUrlLocked = !!envNode;

/**
 * Which network this device acts in. Persisted, because it is not a view
 * preference: it decides which ledger the member's standing lives on, and
 * founding is irreversible — so it is asked once, before the key exists, and
 * changed only deliberately.
 */
export const networkId = writable<string>(envNode ? 'custom' : (lsGet('edet-network') ?? DEFAULT_NETWORK));
/** The URLs behind the `custom` entry (a set, see `networks.ts::splitUrls`);
 *  ignored by every named network. */
export const customNodeUrl = writable<string>(envNode ?? lsGet('edet-node') ?? '');

/** Chain id injected by the launcher beside `EDET_NODE`. */
const envChain: string | undefined = (import.meta as any).env?.EDET_CHAIN || undefined;

/**
 * The chain id behind the `custom` entry — typed by the member, from whoever
 * runs the network, and never read off the node: a node that names the chain
 * chooses which ledger a signature is valid on (`submit.ts::signingChainId`
 * refuses to sign while this is empty). Ignored by every named network, which
 * declares its own in `networks.ts`.
 */
export const customChainId = writable<string>(envChain ?? lsGet('edet-chain') ?? '');

networkId.subscribe((v) => {
    if (!nodeUrlLocked) lsSet('edet-network', v);
});
customNodeUrl.subscribe((v) => {
    if (!nodeUrlLocked) lsSet('edet-node', v);
});
customChainId.subscribe((v) => {
    if (!nodeUrlLocked) lsSet('edet-chain', v);
});

/**
 * Every node of the selected network: the first is read from, the rest are
 * what its answers are compared against. See `lib/networks.ts` for why this
 * is a set and why it is declared there rather than taken from the node.
 */
export const networkNodes = derived([networkId, customNodeUrl], ([$id, $url]) => resolveNodes($id, $url));

/** The node reads go to. Empty until a network with a node is selected, which
 *  the poll treats exactly as an unreachable node. */
export const activeBase = derived(networkNodes, ($nodes) => $nodes[0] ?? '');


// ---------------------------------------------------- authenticated reads ----
// Every read carries a credential from lib/session.ts — whichever one the
// live transport can carry.
//
// Browser: the bearer-token store. The getter always reads the token for the
// CURRENT actor (not whoever minted it), so a stale token from a
// since-abandoned actor is never sent.
//
// One credential for every host: signing a key proof per
// read because IPC carries no headers, and it no longer speaks IPC.
api.configureAuth(
    () => currentToken(get(currentActorId)),
    async () => {
        await remintSession(get(activeBase), get(currentActorId));
    },
);

/**
 * Mint (or refresh) the read-session token whenever the acting identity or
 * the node URL changes — the "one signature at unlock" the session design actually wants.
 * `ensureSession` no-ops (no network, returns null) when this device
 * doesn't hold that member's seed yet, or in Tauri mode; either way reads
 * simply stay anonymous until a token is available, never a hard failure.
 * A previous actor's token is dropped up front: it is bound to their key,
 * not to "whichever member this device is acting as right now".
 */
currentActorId.subscribe((id) => {
    clearSession();
    if (id !== null) void ensureSession(get(activeBase), id);
});
// A token is minted against one specific node, so changing which node this
// device reads — a different network, or a different URL behind `custom` —
// invalidates it just as surely as switching actors does. `activeBase`
// rather than the URL store, because the network entry is what decides it.
activeBase.subscribe(() => {
    clearSession();
    const id = get(currentActorId);
    if (id !== null) void ensureSession(get(activeBase), id);
});

// **Mint the read session once the chain id is known, not before.** A signed
// `/session` request binds to the chain (`session.ts::viewerAuthMessage`), and
// at boot the stored actor is resolved — and its first mint attempted — before
// the first poll has set `viewerChain`. That attempt signs an empty chain, the
// node rejects it, and nothing re-tries: an unauthenticated read returns 200
// (a reduced member view, a `forbidden` pending queue), never the 401 that
// drives the re-mint, so the wallet reads anonymously for the whole session
// and a member's own queue and full record silently never load. The fix is to
// mint again the moment the chain first becomes known. `ensureSession` is a
// no-op when a valid token is already held, so later polls (which re-set the
// same chain) cost nothing. A stale token after a node restart is a different
// path and self-heals: it IS sent as a bearer, the node 401s it, and the read
// layer re-mints.
let lastSessionChain = '';
viewerChain.subscribe((chain) => {
    if (!chain || chain === lastSessionChain) return;
    lastSessionChain = chain;
    const id = get(currentActorId);
    if (id !== null) void ensureSession(get(activeBase), id);
});

// ----------------------------------------------------------------- views ----

/** Read-only federation telemetry: this node's peers and their heads. */
export interface ClusterNode {
    url: string;
    index: number;
    height: number | null;
    hash: string;
    epoch: number | null;
    up: boolean;
    self: boolean;
}

export const clusterNodes = writable<ClusterNode[]>([]);
export const networkView = writable<api.NetworkView | null>(null);
export const membersList = writable<api.MemberSummary[]>([]);
export const contractsList = writable<api.ContractView[]>([]);
/**
 * What this node left out of the two lists above, per path, or `null` where it
 * left nothing out.
 *
 * A node serves at most `MAX_VIEW_ITEMS` rows and SAYS so; a list that showed
 * five hundred of nine hundred without saying so would be a wrong promise, and
 * the member would have no way to tell an absent counterparty from one this
 * node did not send.
 */
export const listTruncation = writable<Record<string, api.Truncation | null>>({});
export const myMember = writable<api.MemberDetail | null>(null);
export const proposalsView = writable<api.ProposalsView | null>(null);
export const paramsView = writable<api.ParamsView | null>(null);
export const pendingView = writable<api.PendingView | null>(null);

/** True once the first successful poll has landed. */
export const firstLoadDone = writable(false);

/** False after two consecutive failed polls of the node. */
export const nodeUp = writable(true);
let failStreak = 0;

/** Signature requests awaiting the current actor. */
export const pendingCount = derived(pendingView, ($p) => $p?.awaiting_me?.length ?? 0);

/**
 * Supporters who have listed the current member and are waiting on their
 * approval.
 *
 * Listing somebody costs them nothing and asks them nothing, so it raises no
 * pending-pool entry and cannot appear under Requests — the approval lives on
 * the Support Circle page alone. That made a decision somebody was waiting on
 * invisible from every other screen, which is the reason this count exists.
 *
 * An ABSENT `supporters` reads as zero rather than as unknown, and here that
 * is the right way round: the field sits behind `full_access`, and a badge is
 * a claim that something is waiting. Saying nothing when we cannot see is
 * honest; the page itself says "not visible here" when you get there.
 */
export const supporterApprovals = derived(
    myMember,
    ($m) => ($m?.supporters ?? []).filter((s) => !s.approved).length,
);

/** Primary on-ledger public key (hex) per member — the claimed-signer set
 *  for dry-run checks of multi-party transactions. */
export const memberKeyOf = derived(membersList, ($members) => {
    const map = new Map<number, string>($members.map((m) => [m.id, m.keys[0]]));
    return (id: number): string | null => map.get(id) ?? null;
});

/**
 * Ask every node of this network the same question, so their answers can be
 * compared. `NetworkStatus` renders the state commitments side by side and a
 * disagreement is the point of the screen.
 *
 * **The peer list comes from `lib/networks.ts`, never from the node.** It used
 * to come from `NetworkView.peers` — from the node being checked — which made
 * the comparison circular: a node that lies can name its own confederates, or
 * name none and be checked against nothing. That was sound telemetry while
 * the app read a node it ran itself; it is not a check on a stranger's node,
 * and a stranger's node is what this client reads now.
 */
async function pollPeers(net: api.NetworkView | null): Promise<void> {
    const urls = get(networkNodes);
    if (urls.length === 0) {
        clusterNodes.set([]);
        return;
    }
    const base = urls[0];
    const nodes = await Promise.all(
        urls.map(async (url, i): Promise<ClusterNode> => {
            // The node reads already went to `base` this tick; asking it twice
            // would only cost a round trip to learn what we hold.
            const self = url === base;
            try {
                const n = self && net ? net : await api.network(url);
                return { url, index: n.index ?? i, height: n.height, hash: n.state_hash, epoch: n.epoch, up: true, self };
            } catch {
                return { url, index: i, height: null, hash: '', epoch: null, up: false, self };
            }
        }),
    );
    clusterNodes.set(nodes);
}

async function pollActive(base: string): Promise<void> {
    const actor = get(currentActorId);
    try {
        const [net, mems, ctrs, props, prm] = await Promise.all([
            api.network(base),
            api.members(base),
            api.contracts(base),
            api.proposals(base),
            api.params(base),
        ]);
        networkView.set(net);
        // Every signature this device makes binds to a chain id, and this is
        // the one place that knows which. See `signingChainId` for why a
        // declared network's answer is checked against the node's rather than
        // taken from it.
        viewerChain.set(net.chain_id ?? '');
        membersList.set(mems);
        contractsList.set(ctrs);
        listTruncation.set({
            '/members': api.lastTruncation('/members'),
            '/contracts': api.lastTruncation('/contracts'),
        });
        proposalsView.set(props);
        paramsView.set(prm);
        if (actor !== null) {
            const [me, pend] = await Promise.all([api.memberDetail(base, actor), api.pendingList(base, actor)]);
            myMember.set((me as any).error ? null : me);
            // A node error is a 200 body of shape `{error}` — never a view. Storing
            // one would put a shape with no `awaiting_me` into `pendingView`, and a
            // consumer that reads `.awaiting_me.length` throws inside a store
            // subscriber, which wedges Svelte's notification queue and freezes the
            // whole UI until a reload. Keep the last good view instead.
            if (!(pend as any)?.error && Array.isArray((pend as any)?.awaiting_me)) pendingView.set(pend);
        } else {
            myMember.set(null);
            pendingView.set(await pollKeyed(base));
        }
        void pollPeers(net);
        failStreak = 0;
        nodeUp.set(true);
        firstLoadDone.set(true);
    } catch {
        failStreak += 1;
        if (failStreak >= 2) nodeUp.set(false);
    }
}

/**
 * The poll for a key that has no account yet.
 *
 * Two questions, in this order. Has the ledger seated this key meanwhile? The
 * first trade lands from the OTHER party's device — the seller completing the
 * code the newcomer showed them, or a member recording a purchase from them —
 * so it is asked on every tick from here, wherever the app happens to be
 * open, and the identity is adopted the moment it answers. Until it does, the
 * pending queue is read BY KEY, since a trade a member opened naming this key
 * is the one thing the pool can hold for it.
 */
async function pollKeyed(base: string): Promise<api.PendingView | null> {
    const seed = get(pendingIdentity);
    if (!seed) return null;
    const key = derivePublicKey(seed);
    const keyHex = bytesToHex(Uint8Array.from(key));
    // Soft on failure: the public reads above already answered this tick, so
    // a refused keyed read is not "the node is down" and must not count
    // toward `nodeUp`; the next tick asks again.
    try {
        const who = await api.whois(base, keyHex, keyProof(key, seed, 'GET', `/whois/${keyHex}`));
        if (who.member !== null && who.member !== undefined) {
            adoptPendingSeed(who.member);
            chooseActor(who.member);
            return null;
        }
        return await api.pendingListByKey(base, keyHex, keyProof(key, seed, 'GET', `/pending/key/${keyHex}`));
    } catch {
        return null;
    }
}

export async function refresh(): Promise<void> {
    const base = get(activeBase);
    // No node selected — `custom` with the URL cleared. Reported as
    // unreachable rather than fetched: an empty base makes every read a
    // RELATIVE URL, which in the app resolves against its own asset origin
    // and fails as a 404 that reads like a broken node instead of an
    // unconfigured one.
    if (!base) {
        nodeUp.set(false);
        firstLoadDone.set(true);
        return;
    }
    await pollActive(base);
}

/**
 * The device's own comment on its binding, kept honest.
 *
 * This instance is bound to ONE community — embedded in Tauri, a fixed URL in
 * the browser — and that is the community it acts in. What it is not bound to
 * is the set of communities it can WATCH: a member of two is exactly what a
 * correspondent is (§Model), and the secret that completes a leg here is published
 * over there. `lib/communities.ts` holds the ones this device watches, and
 * `lib/legwatch.ts` is what does the watching.
 */

let timer: ReturnType<typeof setInterval> | null = null;

export function startPolling(intervalMs = 1500): void {
    stopPolling();
    void refresh();
    timer = setInterval(() => void refresh(), intervalMs);
}

/**
 * Re-pace a poll that is already running, and do nothing otherwise.
 *
 * The cadence is a function of whether anybody is watching the screen
 * (`lib/background.ts`), and that question is asked from a `visibilitychange`
 * handler that can fire before the vault has opened. Starting a poll from
 * there would read the pool with no identity resolved, so this only ever
 * changes the rate of a poll `App.svelte` has already started.
 */
export function setPollInterval(intervalMs: number): void {
    if (timer === null) return;
    startPolling(intervalMs);
}

export function stopPolling(): void {
    if (timer !== null) {
        clearInterval(timer);
        timer = null;
    }
}

// Re-poll immediately when the acting identity or the selected network
// changes. `activeBase` covers both network entries and a custom URL edit.
currentActorId.subscribe(() => {
    if (timer !== null) void refresh();
});
activeBase.subscribe(() => {
    if (timer !== null) void refresh();
});

/**
 * After a submit: polls a few times in quick succession so the committed
 * result shows up as soon as the (150 ms tick) consensus lands it.
 */
export function refreshSoon(): void {
    setTimeout(() => void refresh(), 400);
    setTimeout(() => void refresh(), 1200);
}

// ------------------------------------------------------------- derived -----

/** The current actor's row in the members list (cheap summary). */
export const mySummary = derived(
    [membersList, currentActorId],
    ([$members, $id]) => $members.find((m) => m.id === $id) ?? null,
);
