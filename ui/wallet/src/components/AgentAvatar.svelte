<script lang="ts">
    /**
     * Deterministic identicon for a member: a symmetric 5×5 pixel glyph on a
     * hue derived from the member's wallet address (falling back to the id
     * before the members list has loaded). Pure SVG — no canvas, no external
     * deps, and the same address renders identically on every device.
     */
    import { addressOf } from '../lib/display';

    export let memberId: number;
    export let size = 32;

    const GRID = 5;

    function fnv1a(s: string): number {
        let h = 0x811c9dc5;
        for (let i = 0; i < s.length; i++) {
            h ^= s.charCodeAt(i);
            h = Math.imul(h, 0x01000193);
        }
        return h >>> 0;
    }

    function mulberry32(seed: number): () => number {
        let a = seed >>> 0;
        return function () {
            a |= 0;
            a = (a + 0x6d2b79f5) | 0;
            let t = Math.imul(a ^ (a >>> 15), 1 | a);
            t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
            return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
        };
    }

    function cellsFor(seed: number): boolean[] {
        const rand = mulberry32(seed);
        const cells: boolean[] = new Array(GRID * GRID).fill(false);
        for (let y = 0; y < GRID; y++) {
            for (let x = 0; x <= Math.floor(GRID / 2); x++) {
                const on = rand() > 0.45;
                cells[y * GRID + x] = on;
                cells[y * GRID + (GRID - 1 - x)] = on;
            }
        }
        return cells;
    }

    $: address = $addressOf(memberId);
    $: seed = address ? fnv1a(address) : ((memberId ?? 0) + 1) * 2654435761;
    $: hue = seed % 360;
    $: fg = `hsl(${hue}, 62%, 42%)`;
    $: bg = `hsl(${hue}, 45%, 90%)`;
    $: cells = cellsFor(seed);
</script>

<span
    class="agent-avatar"
    style="width: {size}px; height: {size}px;"
    role="img"
    aria-label={`Avatar for member ${address ?? '#' + memberId}`}
>
    <svg viewBox="0 0 {GRID} {GRID}" width={size} height={size} aria-hidden="true">
        <rect x="0" y="0" width={GRID} height={GRID} fill={bg} />
        {#each cells as on, i}
            {#if on}
                <rect x={i % GRID} y={Math.floor(i / GRID)} width="1" height="1" fill={fg} />
            {/if}
        {/each}
    </svg>
</span>

<style>
    .agent-avatar {
        border-radius: 50%;
        overflow: hidden;
        display: inline-flex;
        align-items: center;
        justify-content: center;
        vertical-align: middle;
        flex-shrink: 0;
        box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.08);
    }
</style>
