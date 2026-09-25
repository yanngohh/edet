/**
 * The networks this client can be pointed at.
 *
 * A network is a NAME, a SET of node URLs and a CHAIN ID — never one URL.
 * That is not a convenience for failover; it is the only thing that makes
 * reading somebody else's node safe to do. `crates/node/src/serve/views.rs::head`
 * publishes the state commitment specifically "so a client can ask two nodes
 * the same question and compare the answers", because otherwise "a member
 * talking to one node has no way to notice it lying, which is a strange
 * property for a BFT system to hand its users."
 *
 * The comparison machinery (`node.ts::pollPeers`) needs a peer list, and a
 * list taken from `NetworkView.peers` — from the node being checked — is
 * circular: it can name its own confederates or none at all. The peers of a
 * network are declared HERE, or typed by the member, out of the node's reach.
 *
 * ## What is NOT in this list
 *
 * There is no `mainnet` entry, and adding one whose hosts do not answer would
 * be worse than the omission: onboarding copy is the specification most people
 * read, and a network list naming a chain that does not exist is a promise
 * this project cannot keep. Entries land here when there are nodes to put in
 * them — a real genesis, authored with `edet-node genesis init` and run by
 * validators who have agreed to run it.
 *
 * Until then a member reaches a network through `custom`, which is a
 * declaration they make themselves: the node URLs and the chain id, both from
 * whoever runs the network and neither from the node.
 */

export interface Network {
    /** Stable id — persisted, and the key for the locale strings. Never shown raw. */
    id: string;
    /**
     * Nodes to read from. The first is the one reads go to; the rest are what
     * its state commitment is compared against (`node.ts::pollPeers`). One
     * entry means no cross-check is possible, which is a property of that
     * network, not a bug in the client.
     */
    nodes: string[];
    /**
     * The chain id every signature on this network binds to
     * (`crates/node/src/block.rs::tx_digest`), declared HERE for the same
     * reason the node list is: the device computes its own signing digest
     * itself, and taking the chain id from the node
     * would hand a hostile one the ability to have a member sign for a
     * DIFFERENT ledger — a signature valid wherever that member's key is also
     * known.
     *
     * Absent on `custom`, where the member's own declaration stands in for it
     * (`declaredChainId`); with none typed the wallet refuses to sign, because
     * the node's own answer is the one thing it may not bind a signature to.
     */
    chainId?: string;
    /** A member supplies the URLs and the chain id; nothing is prefilled. */
    custom?: true;
}

/**
 * `local` is a node the member runs themselves — `edet-node malachite --home
 * DIR --client-port 7001`, which is what `just dev` starts. It is a single
 * node by nature: it is the member's own, so there is nobody to cross-check
 * it against and nothing to gain by it.
 */
export const NETWORKS: Network[] = [
    { id: 'local', nodes: ['http://localhost:7001'], chainId: 'edet-dev' },
    { id: 'custom', nodes: [], custom: true },
];

export const DEFAULT_NETWORK = 'local';

export function networkById(id: string): Network | null {
    return NETWORKS.find((n) => n.id === id) ?? null;
}

/**
 * The URLs a member typed for `custom`, as a set: separated by whitespace or
 * commas, trimmed, in the order given, each once. The first is the one reads
 * go to; the rest are the cross-check.
 */
export function splitUrls(text: string): string[] {
    const seen = new Set<string>();
    for (const raw of text.split(/[\s,]+/)) {
        const url = raw.trim();
        if (url) seen.add(url);
    }
    return [...seen];
}

/**
 * The chain id a signature on this network must bind to, or `null` when
 * nothing declares one — a named network's own, or for `custom` the one the
 * member typed (`node.ts::customChainId`). A `null` is a refusal to sign, not
 * a fallback to the node's answer.
 */
export function declaredChainId(id: string, customChainId = ''): string | null {
    const net = networkById(id);
    if (!net) return null;
    if (net.custom) return customChainId.trim() || null;
    return net.chainId ?? null;
}

/**
 * The URL reads go to, and the URLs its answers are checked against.
 *
 * `custom` carries its URLs in `customUrl`; a named network ignores it, so a
 * member switching back and forth does not have their typed URLs silently
 * become the endpoints of a network that declares its own.
 */
export function resolveNodes(id: string, customUrl: string): string[] {
    const net = networkById(id);
    if (!net) return [];
    if (net.custom) return splitUrls(customUrl);
    return net.nodes;
}
