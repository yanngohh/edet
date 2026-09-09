<script lang="ts">
    /**
     * QR of a wallet address or a signed payload — how a counterparty picks
     * you up by scan.
     *
     * **The canvas is rendered at an INTEGER number of pixels per module, and
     * CSS displays it at the size the caller asked for.** Drawing straight
     * into a `size`-wide canvas is what broke scanning: the wallet's "pay me"
     * QR carries an invitation (~312 bytes → version 11, 61×61 modules) and
     * was drawn into 168 px, which is 2.67 px per module. At a fractional
     * scale `qrcode` rounds each module independently, so some come out two
     * pixels wide and some three, and the phone showing it then upscales that
     * blur to its own pixel density. A decoder needs even modules and about
     * four pixels of them; it had neither, so the code sat in the frame and
     * was never acquired. The bare address (42 bytes → version 3) got 4.26
     * px/module and scanned, which is why it looked intermittent rather than
     * broken.
     *
     * Layout does not move: the backing store grows, the CSS box is `size`.
     */
    import QRCode from 'qrcode';

    export let value: string;
    /** The size the QR OCCUPIES, in CSS pixels. Not the render resolution. */
    export let size = 168;
    /** Error correction. A long payload (a buyer's code) reads better at `L`. */
    export let level: 'L' | 'M' | 'Q' | 'H' = 'M';

    /** Pixels per module in the backing store, at minimum. Four is the floor a
     *  decoder wants; the multiplier below usually lands well above it. */
    const MIN_PX_PER_MODULE = 4;
    /** Backing pixels per CSS pixel, so a 3x handset still downsamples rather
     *  than upscales. Beyond this the canvas costs memory for nothing. */
    const DEVICE_SCALE = 4;

    let canvas: HTMLCanvasElement;

    $: if (canvas && value) void draw(canvas, value, level, size);

    async function draw(el: HTMLCanvasElement, text: string, ecc: typeof level, css: number) {
        // `create` first: the module count is what the scale must divide.
        const modules = QRCode.create(text, { errorCorrectionLevel: ecc }).modules.size;
        const total = modules + 2; // margin: 1 each side
        const scale = Math.max(MIN_PX_PER_MODULE, Math.ceil((css * DEVICE_SCALE) / total));
        try {
            await QRCode.toCanvas(el, text, {
                width: total * scale,
                margin: 1,
                errorCorrectionLevel: ecc,
            });
        } catch (e: unknown) {
            console.error('qr render failed', e);
            return;
        }
        // `qrcode` writes `style.width`/`style.height` at the RENDER size
        // (canvas.js:9), and an inline style beats anything a stylesheet or a
        // `style=` attribute says. So the display size is set back here, after
        // it has drawn — otherwise a 168 px code renders 693 px wide.
        el.style.width = `${css}px`;
        el.style.height = `${css}px`;
    }
</script>

<span class="qr-box" role="img" aria-label={`QR code for ${value}`}>
    <canvas bind:this={canvas} style={`width: ${size}px; height: ${size}px;`} aria-hidden="true"></canvas>
</span>

<style>
    .qr-box {
        display: inline-flex;
        padding: 8px;
        background: #fff;
        border-radius: 8px;
        border: 1px solid rgba(0, 0, 0, 0.12);
        line-height: 0;
    }
    /* `toCanvas` writes the backing store's width/height attributes; these
       keep the DISPLAYED box where the caller put it. */
    canvas {
        display: block;
    }
</style>
