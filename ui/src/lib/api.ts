/**
 * Client for the edet node: HTTP to whichever node the member selected
 * (`lib/networks.ts`), in the browser and in the desktop/mobile app alike.
 *
 * There is one transport. A second would exist only if the app embedded a node and read it
 * over Tauri IPC. Two transports meant two credential shapes, two sets of
 * handlers and two chances for one of them to drift into being the lenient
 * one; the app is a client of a node it does not run now, so there is one.
 *
 * Every write goes through `submitChecked`: the transaction is first dry-run
 * against current state (`/tx/check`), so protocol rejections surface with
 * their `ET-*` code *before* anything is queued — commit-time apply failures
 * are silent from the client's point of view.
 */

// Type-only: erased at compile time, so this does not create a runtime
// import cycle with session.ts (which imports `isTauri` from here).
import type { KeyProof } from "./session";
import { parseProof, verifyProof, type InclusionProof } from "./proof";

export type Key = number[]; // 32 bytes

/**
 * A party to a trade: an account that already exists, or the key of one that
 * comes into existence with the trade itself.
 *
 * Mirrors `crates/state/src/types.rs::Party`. There is no create-account
 * transition: a row is seated by the first bonded trade that
 * names it, so naming a KEY is the deliberate statement "this is a new account
 * if it is not one already" — which is exactly the case `/whois` answers
 * `null` for. Everywhere else, name the id: a member's keys move under
 * rotation and their id never does.
 */
export type Party = { Member: number } | { Key: Key };

export const asMember = (id: number): Party => ({ Member: id });
export const asKey = (key: Key): Party => ({ Key: key });
export const partyMember = (p: Party): number | null => ("Member" in p ? p.Member : null);
export const partyKeyHex = (p: Party): string | null =>
    "Key" in p ? p.Key.map((b) => b.toString(16).padStart(2, "0")).join("") : null;
export const samePartyAs = (a: Party, b: Party): boolean => JSON.stringify(a) === JSON.stringify(b);

// ---------------------------------------------------------------- views ----

/**
 * What `/network` serves. **Every field here must exist in
 * `crates/node/src/serve/views.rs::network`** — nothing in the type system
 * gates that, and it has cost twice: six admission fields the
 * node had never served (one of which silently disabled onboarding), and the
 * paper pass found `phi`, `gauge_g` and `kappa_vol` here, two of them RENDERED
 * as stat cards. `gauge_g` names a macroprudential governor, and
 * the paper and the paper both say there is no such brake in this design —
 * so the client was showing `NaN%` under help text promising the community
 * throttles credit centrally. **A view that describes a mechanism the chain
 * does not run is read as a promise.** `serve/tests.rs` gates the shape at the
 * wire now.
 */
export interface NetworkView {
    index: number;
    n: number;
    height: number;
    /** Every signed envelope is bound to this — see `lib/txdigest.ts`. */
    chain_id: string;
    epoch: number;
    mempool: number;
    members: number;
    active_members: number;
    epoch_secs: number;
    min_maturity_epochs: number;
    /** How far past its acceptance a claim may mature and still be insured
     *  (`ParamKey::InsuredHorizon`). A longer maturity books uninsured however
     *  much capacity carries it, so the trade page reads it beside the
     *  debtor's capacity. */
    insured_horizon_epochs: number;
    theta_adopt: number;
    dust: number;
    v_base: number;
    /** How many members hold a supply. */
    underwriters: number;
    /** What is real: the flow actually drawn through the underwriters. */
    insured_credit: number;
    /**
     * The community's underwriting, all of it seated by a ceremony — genesis
     * or an endorsed amendment.
     *
     * There was a `declared_supply` beside this, carrying an UPPER BOUND
     * rather than a measure: a member whose own capacity came from inside
     * could declare against it, so the declared total inflated geometrically
     * — seed 100, twelve joiners, 204,900 — while what those twelve could owe
     * together stayed at 100. That declaration is refused at the ledger now,
     * so there is one figure and this is it.
     */
    external_seed: number;
    /** What one more epoch of amendments may still admit (§Governance). */
    seed_headroom: number;
    /** Pinned at 1.0 means the ceiling binds and credit is rationed
     *  first-come-first-served. It is NOT a throttle anybody operates. */
    utilisation: number;
    /** Live obligations that fell to the uninsured tier — the figure that tells
     *  an underwriter their seed is small, where utilisation only says it is
     *  full. */
    uninsured_obligations: number;
    validators: number[];
    peers: string[];
    state_hash: string;
}

/** Mirrors `crates/state/src/types.rs::MemberStatus`. There is no admission
 *  ladder: `active` is the resting state of every account that exists, and the
 *  other two are a governance sanction and a voluntary departure. */
export type MemberStatus = 'active' | 'suspended' | 'exited';
export type ContractStatus = 'active' | 'transferred' | 'settled' | 'expired' | 'cured';

/**
 * Authenticated reads (the paper's §Implementation):
 * the node now serves these reputation/risk fields only to an
 * AUTHENTICATED viewer (any member, not just a party — pricing a stranger's
 * risk before transacting is the whole point of exposing them) and omits
 * them entirely — key absent, never zeroed — for an anonymous read. They
 * are `?:` here for exactly that reason: a missing session token (not yet
 * minted, or expired mid-poll) must read as "unknown", not as a zero score.
 * Every consumer (lib/risk.ts, lib/autosign.ts, community UI) must treat
 * `undefined` as "hold"/"don't know", never as "safe".
 */
export interface MemberSummary {
    id: number;
    /** Wallet-like hex handle (0x + 40 hex chars), stable across key rotation. */
    address: string;
    /** Current Ed25519 public keys (hex). */
    keys: string[];
    status: MemberStatus;
    /** What the community will carry this account for: the cut into it. Zero
     *  for an account nobody has staked on, by arithmetic rather than by rule.
     *
     *  **Optional, and absent is not zero.** A cut is a max-flow, so the list
     *  view answers a bounded number of them per read and omits the rest —
     *  and it answers none at all to an unauthenticated caller. Every reader
     *  here already treats a missing risk input as unknown and holds
     *  (`lib/risk.ts`, `lib/autosign.ts`); a zero would read as "nobody backs
     *  them", which is a different and actionable claim. */
    capacity?: number;
    debt: number;
    /** What this member may confer on somebody else: their declared supply if
     *  they underwrite, otherwise their own capacity. A DIFFERENT quantity
     *  from `capacity` — that is how much the community will carry you, this
     *  is how much you may carry others — and the two must not be conflated. */
    conferrable?: number;
    /** Declared supply, present only for an underwriter — and a SCALAR here,
     *  because a list row carries the headline figure only. `MemberDetail`
     *  narrows it to the three-part object the detail view serves. */
    supply?: number;
    open_default?: number;
    /** Who has put standing behind this account, and how much. Public in the
     *  same sense the underwriter set is — a stake is a creditor's recorded
     *  placement, and the whole model rests on standing being legible. */
    backers?: { member: number; amount: number }[];
    /** Advisory-risk inputs (see lib/risk.ts). `capacity` and `debt` above
     *  carry the rest: the score is built on the quantities the cut produces,
     *  so there is no trust mass and no governor brake, and no field here
     *  declares one — `just view-shape-check` holds this type to what the
     *  node serves. */
    d_in?: number;
    d_out?: number;
}

export interface ArbTermsView {
    arbiters: number[];
    quorum: number;
    window_epochs: number;
    award_cap: number;
}

/**
 * Authenticated reads: `debtor`/`creditor` (the graph edge) and the
 * arbitration-tribunal fields are exact only for a party, a named arbiter,
 * or a validator — ABSENT (not zeroed/pseudonymized) otherwise, so a
 * non-party's read is `{ id, outstanding, original, status, ... }` with no
 * counterparty identity at all. Every UI list that iterates `/contracts`
 * unfiltered (as opposed to a `myMember.owes`/`owed` list, always
 * self-party) must filter defensively rather than assume these are set.
 */
export interface ContractView {
    id: number;
    debtor?: number;
    creditor?: number;
    outstanding: number;
    original: number;
    status: ContractStatus;
    maturity_epoch: number;
    created_epoch: number;
    /** The epoch the claim was accepted — the base its insured horizon is
     *  measured from, which a transfer and a routed successor inherit where
     *  `created_epoch` restarts. An extension of an insured claim past
     *  `accepted_epoch + NetworkView.insured_horizon_epochs` drops the
     *  insurance, on the same two signatures. */
    accepted_epoch: number;
    /**
     * Whether the community stands behind this obligation, or the creditor
     * carries it alone (§Recourse).
     *
     * The single most decision-relevant fact about a claim, and it is not
     * inferable from anything else on this record: an insured obligation
     * reserved flow at acceptance and the recourse machinery is behind it; an
     * uninsured one reserves nothing, triggers no community recourse on
     * default, and is the creditor's own risk. Both are ordinary and both are
     * consented to — the uninsured tier is not a fallback but the operating
     * mode of a young community.
     */
    insured: boolean;
    arb?: ArbTermsView | null;
    arb_attested?: number[];
    arb_awarded?: boolean;
}

export interface SupportEdgeView {
    member: number;
    weight: number;
    approved: boolean;
    /**
     * The most this edge can ever route: `ν` times what the pair has staked in
     * one another, and **null toward yourself**, which is a share rather than a
     * relationship.
     *
     * There is deliberately no floor on it (§Standing), so a pair with no settled
     * history between them drains **zero** whatever the weight says and whoever
     * approved whom. The page showed a weight and an approval pill and nothing
     * else, so two members could list, approve, watch both pills go green and
     * route nothing at all, for ever, with no indication why. This is the
     * quantity the mechanism actually reads.
     */
    drain_cap: number | null;
}

export interface GuardianView {
    guardians: number[];
    threshold: number;
    veto_window_epochs: number;
}

/**
 * Authenticated reads: relationship fields (sponsors, guardian,
 * pending_rotation, bonds, the waterfall graph) are exact only for the
 * member themself, a named sponsor/
 * guardian, or a validator — ABSENT otherwise (whole-field-family gate, all
 * or nothing per family; see `crates/node/src/serve/views.rs::member`'s doc
 * comment). `sigma_w` etc inherit `MemberSummary`'s `?:` treatment. `owes`/
 * `owed` are always arrays (never absent), but their `ContractView` entries
 * individually drop `debtor`/`creditor`/`arb*` per the same rule when this
 * viewer isn't a qualifying party to THAT contract (relevant mainly to a
 * validator or named-arbiter viewer browsing someone else's book; a self
 * read always qualifies for its own contracts).
 */
/**
 * Operation-bond position: what this member has reserved against its own
 * write ceiling right now. Optional for the same reason the stake families
 * are — it rides the `full_access` gate, so it is ABSENT unless the viewer is
 * the member themself or a validator.
 *
 * `encumbered` is reserved, never spent: it returns to `headroom` on its own
 * schedule and is paid to nobody, so this is not a fee balance and must not
 * be presented as one. `free_remaining` is the allowance left this epoch,
 * which is what ordinary use actually runs on — a member only ever touches
 * `headroom` after exhausting it.
 */
/** The underwriter side of a member's own detail view. */
export interface UnderwriterView {
    /** What they have undertaken to stand behind — and, since every
     *  declaration is ceremony-seated, the share of the community's external
     *  seed that is theirs. It is what governance weighs, so a member with
     *  `declared === 0` may propose and may not assent. */
    declared: number;
    /** What is drawn through them right now — the floor a withdrawal may not
     *  go below, since the debt did not shrink because they changed their
     *  mind. */
    committed: number;
}

export interface OperationBondView {
    encumbered: number;
    headroom: number;
    unit: number;
    free_remaining: number;
    saturated_epochs: number;
    /** **What this member's standing still carries in NEW ROWS**, in
     *  denomination units: divided by `unit`, how many more newcomers they can
     *  bring into the ledger. A bond is a rate and refills every epoch; a row
     *  is a stock, so seating one holds a bond unit of this for as long as the
     *  row exists and nothing gives it back. It falls only when a row is
     *  seated and rises only when new backing reaches the member. */
    seat_reach: number;
    /** **Has the community put anything behind this account?** — the gate's own
     *  qualification for the free allowance, served rather than re-derived.
     *  Computing it in the client as `conferrable > dust` matches what the
     *  gate did when that line was written; the gate reads the same quantity
     *  GROSS of live credit now, so the two would have disagreed exactly when a
     *  member's backers were busy — the case where being told "nobody is
     *  backing you" is worst. */
    established: boolean;
    /** **The one self-act the seat paid for**, while it is unspent: a newly
     *  seated row may register guardians once on its own key, without a
     *  co-signer, and the gate spends it whether or not the write is
     *  accepted. */
    seat_slot: boolean;
}

/**
 * What a member's own detail view serves. **`supply` is narrowed here**, and
 * the reason is the defect that found it: `MemberSummary` types it as the
 * scalar a list row carries, `MemberDetail` inherited that, and the detail view
 * has always served an object. So the field the governance electorate is read
 * from was both unreachable and mistyped, and the Assent button was offered to
 * everybody because nothing could ask the question. `just view-shape-check`
 * compares types as well as names since.
 */
export interface MemberDetail extends Omit<MemberSummary, 'supply' | 'capacity'> {
    supply?: UnderwriterView;
    /** **Always served here**, unlike on the list. This view answers for ONE
     *  member, so it computes one cut — where the list would answer up to
     *  `MAX_VIEW_ITEMS` of them and omits what it cannot afford. */
    capacity: number;
    joined_epoch: number;
    operation_bond?: OperationBondView;
    is_validator: boolean;
    beneficiaries?: SupportEdgeView[];
    supporters?: SupportEdgeView[];
    guardian?: GuardianView | null;
    pending_rotation?: { opened_epoch: number } | null;
    owes: ContractView[];
    owed: ContractView[];
}

export interface GovernedParamView {
    key: string;
    value: number;
    min: number;
    max: number;
    last_amend_epoch: number | null;
}

export interface ParamsView {
    governed: GovernedParamView[];
    // No admission field of any kind, and the node serves none: an account
    // exists as soon as somebody trades with a key, so "can I join?"
    // is a question with one permanent answer. Three of them were declared
    // here for a trial tier the ledger does not have, so
    // `open_admission` read `undefined` and the wallet's own "join now" path
    // was unreachable — the client describing a rule the ledger never ran,
    // which is the same finding one layer out.
    v_base: number;
    dust: number;
    epoch_secs: number;
    min_maturity_epochs: number;
    insured_horizon_epochs: number;
    gov_cooldown_epochs: number;
    last_redenom_epoch: number | null;
    /**
     * The operation-bond schedule constants (public chain metadata — they
     * apply identically to every member, and `bond_unit` is just
     * `BondFraction * v_base`, both already in this view).
     *
     * `bond_unit` is the amount one unit of transition class reserves;
     * `bond_free_allowance` is how many bonded writes a member gets per
     * epoch before any reservation is made at all; `bond_release_epochs` is
     * how long a reservation is held before it returns; and after
     * `bond_forfeit_epochs` consecutive saturated epochs a member's held
     * bonds become forfeitable by a permissionless crank.
     *
     * Optional: a node from before these were published simply omits them,
     * and the wallet then falls back to describing the mechanism without
     * quoting figures rather than inventing defaults.
     */
    bond_unit?: number;
    bond_free_allowance?: number;
    bond_release_epochs?: number;
    bond_forfeit_epochs?: number;
}

export interface ProposalView {
    id: number;
    author: number;
    kind: Record<string, any> & { type: string };
    assents: number[];
    /// The share of the external seed behind this proposal, in denomination
    /// units — the numerator of the assent measure, computed by the node
    /// against live state.
    assented_seed: number;
    enacted: boolean;
}

export interface ProposalsView {
    proposals: ProposalView[];
    /// The denominator of the assent measure: every external commitment
    /// the community has made, genesis plus every endorsed amendment. NOT a
    /// member count — governance answers to the ceremony, not to a headcount.
    external_seed: number;
    theta_adopt: number;
}

// ------------------------------------------------------------ transport ----

export function isTauri(): boolean {
    return typeof window !== "undefined" && !!((window as any).__TAURI_INTERNALS__ || (window as any).__TAURI__);
}

// ---------------------------------------------------- authenticated reads ----
// the paper's §Implementation.
//
// Browser mode carries a bearer session token (lib/session.ts) on every
// read GET, minted once at unlock/actor-switch and re-minted reactively on a
// 401. Tauri mode has no headers to put a token in, so it carries a
// key-addressed proof as a command argument instead — the same three
// values, the same signed message, verified by the same node-side function.
//
// The proof is built per read because it is bound to that read's PATH, and
// the path an IPC read signs over is the one it has on the HTTP surface
// (`serve::Read` on the node side) — one naming scheme, so a credential does
// not depend on how the request travelled.
//
// api.ts stays decoupled from lib/session.ts (which itself depends on
// lib/actors.ts) via small setters `lib/node.ts` wires once at startup — this
// file has no import of the identity/session layers, so there is no cycle to
// reason about.
type AuthTokenGetter = () => string | null;
type UnauthorizedHandler = () => Promise<void>;
/** Sign a viewer proof for `path` with the acting identity's held seed;
 *  null before unlock (no seed yet), which reads as anonymous. */

let authTokenGetter: AuthTokenGetter = () => null;
let onUnauthorized: UnauthorizedHandler = async () => {};

/** Wire the session-token layer into every read. Call once.
 *
 * There is no third argument here for a per-read key-proof signer, which the
 * IPC transport. Reads carry a bearer token now whatever the host is; the two
 * calls that authenticate by KEY rather than by member id — `whois` and
 * `pendingListByKey` — still pass a proof of their own, in headers, because
 * their caller has no member id to be a session for. */
export function configureAuth(getToken: AuthTokenGetter, handleUnauthorized: UnauthorizedHandler): void {
    authTokenGetter = getToken;
    onUnauthorized = handleUnauthorized;
}

function authedFetch(base: string, path: string, extraHeaders: Record<string, string> = {}): Promise<Response> {
    const token = authTokenGetter();
    const headers: Record<string, string> = { ...extraHeaders };
    // Last word to the bearer token, matching the node's own preference
    // order (`serve::auth::Viewer` checks Bearer first and returns): a live
    // session is the stronger, cheaper credential, and a caller that has one
    // should not be re-verified through a signature on every read.
    if (token) headers['Authorization'] = `Bearer ${token}`;
    return fetch(`${base}${path}`, { headers });
}

/**
 * A read of a PUBLIC endpoint, sent with no credential.
 *
 * `/network` is the same for every caller and gated by nothing, and it is the
 * one read this client makes across NODES — the federation telemetry compares
 * each declared node's head. A session token is minted against ONE node
 * (`activeBase`), so attaching it here sends node A's token to node B, which
 * has never seen it and answers 401; the authenticated `read` then re-mints a
 * fresh node-A token and retries against node B, so every poll produced a 401
 * (and needless churn) on the peer. A public read carries nothing to reject.
 */
async function readPublic<T>(base: string, path: string): Promise<T> {
    const res = await fetch(`${base}${path}`);
    if (!res.ok) throw new Error(`${path}: ${res.status}`);
    return res.json();
}

async function read<T>(
    base: string,
    path: string,
    extraHeaders: Record<string, string> = {},
): Promise<T> {
    // One transport. Two would mean the desktop and mobile app read
    // its own embedded node over Tauri IPC, carrying the viewer credential as
    // a command argument because IPC has no headers. The app embeds no node
    // any more — it is a client of whichever node the member picked — so
    // every read is this fetch, with the same credential the browser sends.
    //
    // The `cmd` and `args` parameters went with the IPC path rather than
    // being left unread: a parameter every caller fills and nothing consumes
    // is a claim about the code that stops being true silently.
    let res = await authedFetch(base, path, extraHeaders);
    if (res.status === 401) {
        // Token unknown/expired (or none minted yet): re-mint and retry once
        // before giving up — the reactive half of the "re-mint on
        // expiry/401" instruction (the proactive half is minting at
        // unlock/actor-switch, see lib/session.ts).
        await onUnauthorized();
        res = await authedFetch(base, path, extraHeaders);
    }
    if (!res.ok) throw new Error(`${path}: ${res.status}`);
    return res.json();
}

export const network = (base: string) => readPublic<NetworkView>(base, "/network");

/** What a node last said it left out of a list, or `null` if it left nothing out. */
export interface Truncation {
    /** Rows this answer actually carried. */
    shown: number;
    /** Rows the node holds. */
    total: number;
    /**
     * The cursor the next page would start after, or `null` when the walk
     * reached the end. Present here means the wallet STOPPED early — it hit
     * its own page cap — so the copy that says what was left out is telling
     * the truth about a bounded walk rather than about one page.
     */
    next: number | null;
}

const truncations: Record<string, Truncation | null> = {};

/**
 * **The node answers a list in two shapes, and both are the same list.**
 *
 * A bare JSON array while the community is small, and
 * `{ <key>, truncated, total }` past the node's `MAX_VIEW_ITEMS` — its way of
 * saying it omitted rows rather than inventing a page nobody asked for. A
 * client typed only for the array does not degrade at that boundary, it
 * STOPS: the store holds an object, and every `.find`, `.filter` and `.map`
 * over it in the app is a `TypeError`, in every wallet reading that node at
 * once.
 *
 * `just view-shape-check` is blind to it — it compares the client's types
 * against the ROW the node serves, and the second shape is a different
 * envelope around the same rows.
 *
 * What the node said it omitted is RECORDED rather than dropped: a list that
 * quietly shows five hundred of nine hundred is a wrong promise, and
 * `lastTruncation` is where a view can find out. Anything neither shape
 * describes — a lying node, a proxy that rewrote the body — reads as an empty
 * list, because a degraded read is a thing the UI already handles and a
 * `TypeError` is not.
 */
function listOf<T>(body: unknown, key: string): { rows: T[]; total: number; next: number | null } {
    if (Array.isArray(body)) {
        return { rows: body as T[], total: body.length, next: null };
    }
    const rows = (body as Record<string, unknown> | null)?.[key];
    if (!Array.isArray(rows)) {
        return { rows: [], total: 0, next: null };
    }
    const total = (body as Record<string, unknown>).total;
    const next = (body as Record<string, unknown>).next;
    return {
        rows: rows as T[],
        total: typeof total === "number" ? total : rows.length,
        next: typeof next === "number" ? next : null,
    };
}

/** What the last read of `path` left out — `null` when it left nothing out. */
export const lastTruncation = (path: string): Truncation | null => truncations[path] ?? null;

/**
 * How many pages a wallet will walk before it stops and says so.
 *
 * Bounded, and the bound is the point: `next` comes from the node, so an
 * unbounded walk is a loop a lying node can start and never end. Eight pages
 * of five hundred is four thousand rows, well past any community this design
 * is for, and past it the member is told what was left out rather than shown a
 * silent prefix.
 */
export const MAX_LIST_PAGES = 8;

/**
 * Walk `path`'s pages until the node says there are no more or the wallet's own
 * page cap stops it, and record what was left out either way.
 *
 * One page is one read at the node's price, so this costs what it asks for.
 */
async function walk<T>(base: string, path: string, key: string, maxPages = MAX_LIST_PAGES): Promise<T[]> {
    const out: T[] = [];
    let after: number | null = null;
    let total = 0;
    let next: number | null = null;
    for (let page = 0; page < maxPages; page++) {
        const q: string = after === null ? "" : `?after=${after}`;
        const body: unknown = await read<unknown>(base, `${path}${q}`);
        const { rows, total: t, next: n }: { rows: T[]; total: number; next: number | null } = listOf<T>(body, key);
        out.push(...rows);
        total = t;
        next = n;
        if (n === null) break;
        after = n;
    }
    truncations[path] = next === null ? null : { shown: out.length, total, next };
    return out;
}

export const members = (base: string): Promise<MemberSummary[]> =>
    walk<MemberSummary>(base, "/members", "members");
export const memberDetail = (base: string, id: number) =>
    read<MemberDetail>(base, `/member/${id}`);
export const contracts = (base: string): Promise<ContractView[]> =>
    walk<ContractView>(base, "/contracts", "contracts");
export const params = (base: string) => read<ParamsView>(base, "/params");
export const proposals = (base: string) => read<ProposalsView>(base, "/proposals");

/**
 * Which member holds this public key or address (hex, no `0x` for a key)?
 * The restore-from-phrase and await-admission lookup.
 *
 * The node only resolves this for the holder themself (or a validator), and
 * a caller in either of those flows has no member id yet — so it cannot use
 * the id-addressed header or a bearer token, and passes a key-addressed
 * proof instead (`lib/session.ts::keyProof`). Without one the lookup is
 * anonymous and the node answers `forbidden` for anything that does resolve.
 *
 * The proof is additive, not a replacement: callers that DO have a session
 * (resolving a counterparty, say) keep using it, and the node prefers a
 * bearer token when both arrive. Onboarding simply has no token to send.
 */
export interface WhoisResult {
    member: number | null;
    address?: string | null;
    error?: string;
}

export function whois(base: string, keyHex: string, proof?: KeyProof): Promise<WhoisResult> {
    return read<WhoisResult>(
        base,
        `/whois/${keyHex}`,
        // `keyHex` in the PATH is the NEEDLE; the headers below carry the
        // CALLER. A member resolving a counterparty proves their own key, not
        // the one they are asking about — and passing `proof` explicitly is
        // what lets onboarding present a proof over a key whose member id it
        // does not yet know.
        proof
            ? {
                  // Mirrors the constants in `crates/node/src/serve/auth.rs`.
                  // A mismatch fails closed — the node reads no credential
                  // and answers `forbidden` — never open.
                  "x-edet-viewer-key": proof.key,
                  "x-edet-viewer-sig": proof.sig,
                  "x-edet-viewer-ts": String(proof.ts),
                  "x-edet-viewer-nonce": proof.nonce,
              }
            : {},
    );
}

// ------------------------------------------------------ pending proposals ---

/** A multi-party transaction collecting signatures on the node. */
export interface PendingEntryView {
    digest: string;
    tx: Record<string, any>;
    /**
     * The envelope fields fixed by whoever OPENED this proposal. A
     * co-signer's client MUST read these back from here rather than
     * generate its own — the node recomputes the digest it verifies a
     * signature against from `(tx, nonce, not_after_epoch)`, so a co-sign
     * carrying a different nonce silently lands in a NEW (never-completing)
     * pool entry instead of this one. See `lib/submit.ts::signPending`.
     */
    nonce: number[];
    not_after_epoch: number;
    required: Party[];
    min_sigs: number;
    initiator: number;
    /** Who opened it: the initiator, or the invited KEY whose first purchase
     *  this is, in which case `initiator` is the inviting member the entry
     *  is charged to. */
    opener?: Party;
    created_secs: number;
    signed_by: Party[];
}

export interface PendingView {
    awaiting_me: PendingEntryView[];
    mine: PendingEntryView[];
}

export const pendingList = (base: string, member: number) =>
    read<PendingView>(base, `/pending/${member}`);

/**
 * The same queue for a device the ledger cannot name yet: a key whose first
 * trade is still waiting on this device's signature.
 *
 * A separate call rather than a parameter, because it authenticates
 * differently — there is no member id to compare against, so possession of
 * THIS key is the whole of the authorisation, and the node's `ViewerParty`
 * exists for exactly this one read.
 */
export function pendingListByKey(base: string, keyHex: string, proof: KeyProof): Promise<PendingView> {
    return read<PendingView>(
        base,
        `/pending/key/${keyHex}`,
        // Here the needle and the caller are the SAME key, which is the whole
        // shape of this read: there is no id to compare against, so proving
        // possession of the key IS the authorisation.
        {
            "x-edet-viewer-key": proof.key,
            "x-edet-viewer-sig": proof.sig,
            "x-edet-viewer-ts": String(proof.ts),
            "x-edet-viewer-nonce": proof.nonce,
        },
    );
}

/**
 * A member's signed invitation to be bought from
 * (`serve/pending.rs::Invite`): what lets a key with no account open its
 * own first purchase in the pool, charged to the inviting member.
 */
export interface InviteWire {
    key: Key;
    not_after_secs: number;
    nonce: number[];
    signature: number[];
}

export interface PendingSignReq {
    tx: Record<string, unknown>;
    /** Fixed by the initiator, reused byte-for-byte by every co-signer
     *  — see `PendingEntryView`'s doc comment. */
    nonce: number[];
    not_after_epoch: number;
    required: Party[];
    min_sigs: number;
    signer: Key;
    signature: number[];
    /** Carried only by a key with no account opening its own purchase. */
    invite?: InviteWire;
}

export async function pendingSign(
    base: string,
    req: PendingSignReq,
): Promise<{ ok: boolean; completed?: boolean; queued?: boolean; hash?: string | null; error?: string }> {
    const res = await fetch(`${base}/pending/sign`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(req),
    });
    if (!res.ok) throw new Error(`pending/sign: ${res.status}`);
    return res.json();
}

export async function pendingDecline(
    base: string,
    req: { digest: string; signer: Key; signature: number[] },
): Promise<{ ok: boolean }> {
    const res = await fetch(`${base}/pending/decline`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(req),
    });
    if (!res.ok) throw new Error(`pending/decline: ${res.status}`);
    return res.json();
}


// -------------------------------------------------------------- signing ----

import { derivePublicKey, hexToBytes, randomNonce, signDigest } from "./crypto";

export interface SignedTx {
    tx: Record<string, unknown>;
    /** Makes an otherwise byte-identical resubmission of `tx` a distinct
     *  signed envelope. Client-chosen (CSPRNG), never derived. */
    nonce: number[];
    /** Last epoch this envelope may still apply in (inclusive). */
    not_after_epoch: number;
    signers: Key[];
    signatures: number[][];
}

/** A transaction plus the member ids whose signatures it requires. */
/**
 * The transaction alphabet, mirroring `crates/state/src/tx.rs` variant for
 * variant.
 *
 * Typed, and that is the whole point of it existing. A `Record<string,
 * unknown>` lets the client build transitions the protocol does not have —
 * `Vouch`, `AdmitMember`, `PromoteMember` survive a whole model change in the
 * UI without a single compile error, and a member clicking them has their
 * transaction refused at decode by every node on the network. A union costs one place to update when the alphabet
 * changes, and pays for itself the first time the alphabet changes.
 *
 * Field names are the wire names (`bincode`/`serde` on the node side), so this
 * is deliberately not camelCase: it is a serialization contract, not an
 * ergonomic API. The builders below are where the ergonomics live.
 */
export type Tx =
    | { DeclareSupply: { member: number; supply: number } }
    | {
          Accept: {
              debtor: Party;
              creditor: Party;
              amount: number;
              maturity_epochs: number;
              arb: ArbTermsView | null;
          };
      }
    | { Transfer: { contract: number; new_debtor: number } }
    | { Settle: { contract: number; amount: number } }
    | { Extend: { contract: number; new_maturity_epoch: number } }
    | { MarkExpired: { contract: number } }
    | { Cure: { contract: number; amount: number } }
    | { ArbAttest: { contract: number; arbiter: number; amount: number } }
    | { Sale: { seller: Party; buyer: Party; amount: number; maturity_epochs: number } }
    | { Exit: { member: number } }
    | {
          RegisterGuardians: {
              member: number;
              guardians: number[];
              threshold: number;
              veto_window_epochs: number;
          };
      }
    | { RotateRequest: { member: number; new_keys: Key[] } }
    | { RotateVeto: { member: number } }
    | { RotateFinalize: { member: number } }
    | { ListBeneficiaries: { supporter: number; entries: [number, number][] } }
    | { ApproveSupporter: { beneficiary: number; supporter: number; approved: boolean } }
    | { Propose: { author: number; kind: Record<string, unknown> } }
    | { Assent: { member: number; proposal: number } }
    | { ForfeitBonds: { member: number } };

export interface TxPlan {
    tx: Tx;
    /**
     * The parties whose signatures this transaction wants.
     *
     * Parties rather than member ids: a trade may name a
     * counterparty by KEY, and the key's holder is exactly who has to sign for
     * it. Every other builder names members, and `asMember` is the whole of the
     * difference at those call sites.
     */
    signers: Party[];
    /** Distinct signers needed (threshold actions); default = all of them. */
    minSigs?: number;
}

/** Rejection from the node's dry-run check, carrying the protocol code. */
export class TxRejectedError extends Error {
    code: string;
    constructor(code: string) {
        super(`transaction rejected: ${code}`);
        this.name = "TxRejectedError";
        this.code = code;
    }
}

/**
 * **No signing payload is fetched from a node here, and there must never be
 * one.**
 *
 * A `txDigest` that asked the node's digest route and returned the bytes to
 * sign is justified only by "the embedded node IS the wallet's own code", and
 * this client embeds no node: it reads one the member chooses, and a node
 * reached through `custom` can answer with the digest of a different envelope
 * and collect a valid signature over it.
 *
 * The digest is computed on the device (`lib/txdigest.ts`), cross-pinned to
 * `block.rs::tx_digest` by generated vectors. The node's route exists for the
 * e2e harnesses in `ui/scripts/`, which are not wallets and hold no member's
 * seed; `blind-signing.test.ts` gates that no client module reaches for it.
 *
 * For the same reason there is no `signPlan` or `submitChecked` taking a
 * `base` to fetch a digest through: a signing path nothing calls is a signing
 * path nobody reviews.
 */

/**
 * Sign a plan over a known digest (pure — unit-testable). `nonce` and
 * `notAfterEpoch` are the envelope fields the digest was computed over
 * (C2) — the caller fixes them ONCE per signing intent (see `randomNonce`'s
 * doc comment) and every co-signer of the same intent must pass the SAME
 * values back in, never regenerate them.
 */
export function signPlanWithDigest(
    plan: TxPlan,
    digest: Uint8Array,
    nonce: number[],
    notAfterEpoch: number,
    seedOf: (party: Party) => Uint8Array,
): SignedTx {
    const seen = new Set<string>();
    const signers: Key[] = [];
    const signatures: number[][] = [];
    for (const party of plan.signers) {
        const tag = JSON.stringify(party);
        if (seen.has(tag)) continue;
        seen.add(tag);
        const seed = seedOf(party);
        signers.push(derivePublicKey(seed));
        signatures.push(signDigest(digest, seed));
    }
    return { tx: plan.tx, nonce, not_after_epoch: notAfterEpoch, signers, signatures };
}

/**
 * What a transition reserves, quoted by the node from the SAME schedule its
 * gate enforces (`edet_state::bond::bond_multiple`) instead of a copy of the
 * table kept here — a second copy would drift the moment the schedule moved,
 * and a wallet quoting a stale figure is worse than one quoting none.
 *
 * A bond is reserved, never collected: `amount` is encumbered against the
 * submitter's OWN capacity and released after `release_epochs`, credited to
 * nobody. It is not a fee and must never be presented as one — the ledger
 * charges none, and no party is ever paid for another member's traffic.
 *
 * Only the public half is reported. Whether you actually reserve anything
 * depends on your remaining free allowance and headroom, which `/tx/check`
 * deliberately does not carry — it takes no viewer, so reporting them would
 * make it a probe for any member's write ceiling. Read those off your own
 * `MemberDetail.operation_bond`, which is gated.
 *
 * `amount === 0` is a structural guarantee rather than a discount: settling,
 * curing, transferring, exiting, releasing a vouch and the permissionless
 * cranks are priced at zero forever, so a member can always settle its way
 * out of a default it could otherwise not afford to leave.
 */
export interface TxBondView {
    amount: number;
    release_epochs: number;
}

/** Result of the node's dry-run, with the bond that transition would reserve. */
export interface CheckResult {
    ok: boolean;
    code?: string;
    /** Absent from a node predating the bond disclosure. */
    bond?: TxBondView;
}

/**
 * Dry-run a draft against the node's current state.
 *
 * Authenticated, like a read: the node discloses the VERDICT only to a party
 * to the transaction, because the rejection code is otherwise a probe of the
 * named debtor's private headroom (`ET-CAP-001` fires exactly at their
 * ceiling). An unauthenticated call still gets the bond quote — which is
 * public — but no `ok`, so a caller that skips the credential sees every
 * write it pre-flights read as a failure rather than as a refusal to answer.
 */
export async function checkTx(base: string, signed: SignedTx): Promise<CheckResult> {
    // The pre-flight is a READ that happens to be a POST, and it must carry
    // the viewer credential: answered anonymously the node returns the public
    // bond quote with no `ok`, and the UI reports ET-UNKNOWN for every action.
    // It cost that once already, on the IPC transport, for a different reason
    // (the proof was signed over POST where the node canonicalized to GET).
    const token = authTokenGetter();
    const headers: Record<string, string> = { "content-type": "application/json" };
    if (token) headers['Authorization'] = `Bearer ${token}`;
    const res = await fetch(`${base}/tx/check`, {
        method: "POST",
        headers,
        body: JSON.stringify(signed),
    });
    if (!res.ok) throw new Error(`check: ${res.status}`);
    return res.json();
}

/** A record's inclusion proof, with the root and height it is against. */
export interface ProofReply {
    height: number;
    app_hash: string;
    /** The verified proof, or null — see `fetchProof` for what null means. */
    proof: InclusionProof | null;
}

/**
 * Fetch a record's inclusion proof and **verify it before returning it**.
 *
 * The verification is the point. A proof the client accepts on the node's say-so
 * is worth exactly what the node's unverified JSON is worth, which is what the
 * inclusion proof
 * exists to fix: this is the one read whose answer a member can keep, hand to
 * an arbitrator who has never run edet, and re-check later against a published
 * root. So a reply that does not verify against the root it arrived with is
 * discarded here rather than surfaced — the node is either buggy or lying, and
 * neither is something to render.
 *
 * `proof: null` therefore means one of three things, all of which the caller
 * should treat as "no evidence": the record is not in that section, the node
 * refused (a proof discloses the whole record, so it needs the same access the
 * record does), or what came back failed to verify.
 */
export async function fetchProof(
    base: string,
    kind: 'member' | 'contract',
    id: number,
): Promise<ProofReply | null> {
    const path = `/proof/${kind}/${id}`;
    const body = await read<Record<string, unknown>>(base, path);
    if (!body || typeof body !== 'object' || body.error !== undefined) return null;
    const app_hash = typeof body.app_hash === 'string' ? body.app_hash : '';
    const height = typeof body.height === 'number' ? body.height : 0;
    const proof = parseProof(body.proof);
    if (!proof || !app_hash || !verifyProof(app_hash, proof)) {
        return { height, app_hash, proof: null };
    }
    return { height, app_hash, proof };
}

export type TxOutcomeStatus = 'ok' | 'rejected' | 'pending' | 'unknown';

/**
 * Whatever this replica knows about a submitted transaction, by its content
 * hash (`SignedTx::hash()` on the node — sha256 of the bincode-encoded
 * signed transaction, distinct from the signing payload `lib/txdigest.ts`
 * computes): `"ok"`
 * once it committed and applied cleanly, `"rejected"` with the ET code if
 * it committed but was rejected at apply-time, `"pending"` while still
 * queued/uncommitted, or `"unknown"` otherwise (bad hash, never seen, or
 * evicted from the node's bounded outcome window). See
 * `crates/node/src/serve/views.rs::tx_outcome`.
 */
export interface TxOutcomeView {
    status: TxOutcomeStatus;
    code?: string;
}

/**
 * Poll `GET /tx/outcome/:hash` — the one place a commit-time apply failure
 * (H4) becomes visible, since `submitTx`'s `queued` only reports whether the
 * ingress accepted the transaction, never whether it later stuck.
 *
 * Served over the one transport. An in-app mode reporting `"unknown"`
 * unconditionally because no IPC command existed, so a desktop transaction
 * that committed and was then rejected by `apply` looked exactly like one
 * still in flight.
 */
export const txOutcome = (base: string, hashHex: string) =>
    read<TxOutcomeView>(base, `/tx/outcome/${hashHex}`);

/**
 * `hash` is the key `/tx/outcome/:hash` answers under — the node computes it
 * over the consensus encoding of the signed envelope, which this client does
 * not reproduce — so a caller that wants to learn whether a queued
 * transaction then committed polls it (`submit.ts::watchOutcome`).
 */
export async function submitTx(base: string, signed: SignedTx): Promise<{ queued: boolean; hash?: string | null }> {
    const res = await fetch(`${base}/tx`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(signed),
    });
    if (!res.ok) throw new Error(`submit: ${res.status}`);
    return res.json();
}

// ------------------------------------------------------------- builders ----
// One builder per Tx kind: the JSON matches the node's serde encoding of
// `edet_state::Tx`, and `signers` matches apply.rs's require_signed calls.
// Every builder but the two trade ones names members (`asMember`); `Accept` and
// `Sale` take parties, because either side of a trade may be a key with no
// account yet and that trade is what seats it.

/**
 * Every builder below that takes a `ContractView` is only ever called on a
 * contract the acting member is already debtor or creditor on (they come
 * from `myMember.owes`/`owed`, or a `contractsList` row already filtered to
 * `c.debtor === me || c.creditor === me` — see Transactions.svelte,
 * MyContracts.svelte, Receivables.svelte). Authenticated reads
 * guarantee `debtor`/`creditor` are populated for exactly that case — they
 * are only ever ABSENT for a non-party's read of someone else's contract,
 * which this app never builds a signing plan against. This throws rather
 * than silently building a plan with `undefined` signer ids if that
 * invariant is ever violated (a stale/foreign `ContractView` reaching a
 * builder some other way).
 */
function party(c: ContractView): { debtor: number; creditor: number } {
    if (c.debtor === undefined || c.creditor === undefined) {
        throw new Error('contract counterparty identity is not visible to this viewer');
    }
    return { debtor: c.debtor, creditor: c.creditor };
}

export const tx = {
    // Contracts (bilateral arrows are co-signed).
    /**
     * Book an obligation. Either side may be a `Party::Key`, which is how a
     * newcomer's first trade seats their account — the counterparty carries
     * the bond for the row, which is the same first risk §Recourse already names.
     */
    accept(p: {
        debtor: Party;
        creditor: Party;
        amount: number;
        maturityEpochs: number;
        arb?: ArbTermsView | null;
    }): TxPlan {
        return {
            tx: {
                Accept: {
                    debtor: p.debtor,
                    creditor: p.creditor,
                    amount: p.amount,
                    maturity_epochs: p.maturityEpochs,
                    arb: p.arb ?? null,
                },
            },
            signers: [p.debtor, p.creditor],
        };
    },
    sale(p: { seller: Party; buyer: Party; amount: number; maturityEpochs: number }): TxPlan {
        return {
            tx: { Sale: { seller: p.seller, buyer: p.buyer, amount: p.amount, maturity_epochs: p.maturityEpochs } },
            signers: [p.seller, p.buyer],
        };
    },
    settle(c: ContractView, amount: number): TxPlan {
        return { tx: { Settle: { contract: c.id, amount } }, signers: [asMember(party(c).debtor), asMember(party(c).creditor)] };
    },
    extend(c: ContractView, newMaturityEpoch: number): TxPlan {
        return {
            tx: { Extend: { contract: c.id, new_maturity_epoch: newMaturityEpoch } },
            signers: [asMember(party(c).debtor), asMember(party(c).creditor)],
        };
    },
    /**
     * Move the debtor of a claim.
     *
     * The CREDITOR co-signs only when the successor would be uninsured — a
     * transfer discharges the old debtor, and every discharge is authorised by
     * the party who loses if it is wrong, but the creditor only loses
     * something when the claim stops being one the community stands behind.
     * The node decides which case it is from the graph; the client cannot, so
     * it always collects the creditor's signature and lets the extra one be
     * harmless. Over-collecting is safe; under-collecting is a refused
     * transaction the member cannot diagnose.
     */
    transfer(c: ContractView, newDebtor: number): TxPlan {
        return {
            tx: { Transfer: { contract: c.id, new_debtor: newDebtor } },
            signers: [asMember(party(c).debtor), asMember(party(c).creditor), asMember(newDebtor)],
        };
    },
    markExpired(c: ContractView): TxPlan {
        // Permissionless default crank.
        return { tx: { MarkExpired: { contract: c.id } }, signers: [] };
    },
    cure(c: ContractView, amount: number): TxPlan {
        return { tx: { Cure: { contract: c.id, amount } }, signers: [asMember(party(c).debtor), asMember(party(c).creditor)] };
    },
    arbAttest(c: ContractView, arbiter: number, amount: number): TxPlan {
        return { tx: { ArbAttest: { contract: c.id, arbiter, amount } }, signers: [asMember(arbiter)] };
    },

    // Membership and underwriting
    /**
     * Lower or leave the underwriter role — an accepted liability rather than
     * a rank or a privilege (§Standing, §Adoption).
     *
     * It cannot RAISE a declaration at any capacity: a supply is a promise
     * from outside the community, and standing the community itself conferred
     * cannot stand in for one. Supplies rise through a ceremony — genesis, or
     * a seed amendment carried through governance (§Governance). LOWERING is
     * floored at the flow already committed through them (§Stability): the
     * debt did not shrink because the underwriter changed their mind. `supply:
     * 0` from an underwriter carrying nothing leaves the role.
     */
    declareSupply(member: number, supply: number): TxPlan {
        return { tx: { DeclareSupply: { member, supply } }, signers: [asMember(member)] };
    },
    /** The permissionless forfeiture crank for a member in sustained exhaustion. */
    forfeitBonds(member: number): TxPlan {
        return { tx: { ForfeitBonds: { member } }, signers: [] };
    },
    exit(member: number): TxPlan {
        return { tx: { Exit: { member } }, signers: [asMember(member)] };
    },

    // Guardians / key rotation
    registerGuardians(p: { member: number; guardians: number[]; threshold: number; vetoWindowEpochs: number }): TxPlan {
        return {
            tx: {
                RegisterGuardians: {
                    member: p.member,
                    guardians: p.guardians,
                    threshold: p.threshold,
                    veto_window_epochs: p.vetoWindowEpochs,
                },
            },
            signers: [asMember(p.member)],
        };
    },
    rotateRequest(member: number, newKeys: Key[], guardians: number[], threshold: number): TxPlan {
        return {
            tx: { RotateRequest: { member, new_keys: newKeys } },
            signers: guardians.map(asMember),
            minSigs: Math.max(1, Math.min(threshold, guardians.length)),
        };
    },
    rotateVeto(member: number): TxPlan {
        return { tx: { RotateVeto: { member } }, signers: [asMember(member)] };
    },
    rotateFinalize(member: number): TxPlan {
        return { tx: { RotateFinalize: { member } }, signers: [] };
    },

    // Support cascade
    listBeneficiaries(supporter: number, entries: [number, number][]): TxPlan {
        return { tx: { ListBeneficiaries: { supporter, entries } }, signers: [asMember(supporter)] };
    },
    approveSupporter(beneficiary: number, supporter: number, approved: boolean): TxPlan {
        return { tx: { ApproveSupporter: { beneficiary, supporter, approved } }, signers: [asMember(beneficiary)] };
    },

    // Governance. `kind` uses serde's external tagging of ProposalKind.
    propose(author: number, kind: Record<string, unknown>): TxPlan {
        return { tx: { Propose: { author, kind } }, signers: [asMember(author)] };
    },
    assent(member: number, proposal: number): TxPlan {
        return { tx: { Assent: { member, proposal } }, signers: [asMember(member)] };
    },
};

export const proposalKinds = {
    paramChange(key: string, value: number) {
        return { ParamChange: { key, value } };
    },
    redenominate(num: number, den: number) {
        return { Redenominate: { num, den } };
    },
    suspend(member: number) {
        return { Suspend: { member } };
    },
    unsuspend(member: number) {
        return { Unsuspend: { member } };
    },
    validatorPower(member: number, power: number) {
        return { ValidatorPower: { member, power } };
    },
    // A seed amendment names no beneficiary: it is the AUTHOR's own external
    // commitment, endorsed by the community (the paper's §Governance). A supply is a
    // standing consent to inherit debts, so one that named somebody else would
    // volunteer a member to underwrite — which is why there is no member
    // argument here to pass by mistake.
    seedAmendment(amount: number) {
        return { SeedAmendment: { amount } };
    },
};
