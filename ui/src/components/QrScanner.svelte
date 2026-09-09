<script lang="ts">
    /**
     * Camera QR scanner overlay (html5-qrcode, dynamically imported so the
     * library only loads when a scan is requested). Emits `scanned` with
     * the decoded text, `close` on dismiss. Falls back gracefully where no
     * camera is available — manual entry always remains.
     */
    import { _ } from 'svelte-i18n';
    import { createEventDispatcher, onDestroy, onMount } from 'svelte';
    import Button, { Label } from '@smui/button';

    const dispatch = createEventDispatcher<{ scanned: string; close: void }>();

    /** What the frame is for; the default names an address, the common case. */
    export let title = '';

    let error = '';
    let scanner: { stop(): Promise<void>; clear(): void } | null = null;
    let stopped = false;

    onMount(async () => {
        try {
            const { Html5Qrcode } = await import('html5-qrcode');
            // The platform's own detector where there is one — Android's
            // WebView has `BarcodeDetector` and it is markedly more tolerant
            // of blur and angle than the bundled ZXing port. The library
            // feature-detects and falls back on its own, so this is free.
            const instance = new Html5Qrcode('edet-qr-reader', {
                verbose: false,
                useBarCodeDetectorIfSupported: true,
            });
            scanner = instance;
            await instance.start(
                // EXACTLY one key here, and only `facingMode` or `deviceId`:
                // `createVideoConstraints` throws on anything else, and the
                // throw reaches the member as "Camera unavailable". The
                // resolution request goes in `videoConstraints` below, which
                // the library uses INSTEAD of this argument when it is valid —
                // so that one has to name the camera itself.
                { facingMode: 'environment' },
                {
                    fps: 10,
                    // Ask for a real sensor resolution. The default stream is
                    // whatever the browser picks, often 640x480, and the decode
                    // budget is pixels per QR module: a 61-module code across a
                    // 480-line frame has nothing left to lose. `ideal`, not
                    // `exact`, so a device that cannot do this still opens.
                    videoConstraints: {
                        facingMode: 'environment',
                        width: { ideal: 1920 },
                        height: { ideal: 1080 },
                    },
                    // A FRACTION of the viewfinder, never a fixed box. `qrbox`
                    // CROPS the frame before decoding, so 220 px was a hard
                    // ceiling on resolution no camera could get past: the code
                    // sat in the frame, sharp, and the decoder was handed a
                    // 220 px thumbnail of it.
                    qrbox: (w: number, h: number) => {
                        const side = Math.max(200, Math.floor(Math.min(w, h) * 0.85));
                        return { width: side, height: side };
                    },
                },
                (text: string) => {
                    if (stopped) return;
                    stopped = true;
                    void instance.stop().catch(() => {});
                    dispatch('scanned', text);
                },
                () => {
                    /* per-frame decode misses are normal */
                },
            );
        } catch (e) {
            error = e instanceof Error ? e.message : String(e);
        }
    });

    onDestroy(() => {
        if (scanner && !stopped) {
            stopped = true;
            void scanner.stop().catch(() => {});
        }
    });
</script>

<div class="scan-overlay" role="dialog" aria-modal="true" aria-label={title || $_('scanner.title', { default: 'Scan a wallet address' })}>
    <div class="scan-shell">
        <h3 class="scan-title">{title || $_('scanner.title', { default: 'Scan a wallet address' })}</h3>
        <div id="edet-qr-reader" class="scan-view"></div>
        {#if error}
            <p class="scan-error">
                {$_('scanner.error', {
                    values: { message: error },
                    default: `Camera unavailable (${error}) — type or paste the address instead.`,
                })}
            </p>
        {/if}
        <div class="scan-actions">
            <Button variant="outlined" on:click={() => dispatch('close')}>
                <Label>{$_('common.cancel', { default: 'Cancel' })}</Label>
            </Button>
        </div>
    </div>
</div>

<style>
    .scan-overlay {
        position: fixed;
        inset: 0;
        z-index: 6000;
        background: rgba(0, 0, 0, 0.6);
        display: flex;
        align-items: center;
        justify-content: center;
        padding: 16px;
    }
    .scan-shell {
        background: var(--mdc-theme-surface, #fff);
        border-radius: 12px;
        padding: 16px;
        /* The crop is a fraction of the viewfinder, so the viewfinder's size
           is the decoder's resolution budget. */
        width: min(560px, 100%);
        display: flex;
        flex-direction: column;
        gap: 12px;
    }
    :global(.dark-theme) .scan-shell {
        background: #1e1e1e;
    }
    .scan-title {
        margin: 0;
        color: var(--mdc-theme-primary);
        font-size: 1.1rem;
    }
    .scan-view {
        width: 100%;
        min-height: 320px;
        border-radius: 8px;
        overflow: hidden;
    }
    .scan-error {
        margin: 0;
        color: var(--mdc-theme-error, #d32f2f);
        font-size: 0.88rem;
        line-height: 1.4;
    }
    .scan-actions {
        display: flex;
        justify-content: flex-end;
    }
</style>
