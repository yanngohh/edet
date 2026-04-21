<script lang="ts">
    import { onDestroy, createEventDispatcher } from 'svelte';
    import Dialog, { Content, Title, Actions } from '@smui/dialog';
    import Button, { Label } from '@smui/button';
    import CircularProgress from '@smui/circular-progress';
    import { _ } from 'svelte-i18n';
    import { tauriBridge } from '../../common/tauriBridge';

    export let open = false;

    const dispatch = createEventDispatcher();
    const scannerId = 'qr-reader';

    let html5QrCode: any = null;
    let imgSrc = '';
    let tauriUnlisten: (() => void) | null = null;
    let active = false;
    let error: string | null = null;
    let decoding = false; // throttle: only one scanFileV2 at a time
    let useWebFallback = false; // true when Android forces us to web path

    $: handleOpenChange(open);

    async function handleOpenChange(isOpen: boolean) {
        if (isOpen && !active) {
            error = null;
            imgSrc = '';
            active = true;
            if (tauriBridge.isTauri()) {
                await startTauriScan();
            } else {
                await startWebScan();
            }
        } else if (!isOpen && active) {
            await doStop();
        }
    }

    // ── Tauri path ─────────────────────────────────────────────────────────────
    // Native camera (nokhwa) sends JPEG frames; we display them and feed each
    // one to html5-qrcode's image scanner (ZXing WASM) for decoding.

    function tauriListen(event: string, handler: (payload: any) => void): () => void {
        const internals = (globalThis as any).__TAURI_INTERNALS__;
        if (!internals) return () => {};
        let unlistenFn: (() => void) | null = null;
        internals.invoke('plugin:event|listen', {
            event,
            target: { kind: 'Any' },
            handler: internals.transformCallback((ev: { payload: any }) => handler(ev.payload)),
        }).then((id: number) => {
            unlistenFn = () =>
                internals.invoke('plugin:event|unlisten', { event, eventId: id });
        });
        return () => { unlistenFn?.(); };
    }

    async function startTauriScan() {
        const { Html5Qrcode } = await import('html5-qrcode');
        // Use a hidden element — scanFileV2 needs an elementId for its canvas.
        html5QrCode = new Html5Qrcode('qr-decode-canvas');

        const ulFrame = tauriListen('qr://frame', async (b64: string) => {
            // Always update preview.
            imgSrc = `data:image/jpeg;base64,${b64}`;

            // Skip if a decode is already in flight.
            if (decoding) return;
            decoding = true;
            try {
                // Convert base64 → Blob → File, then let html5-qrcode decode it.
                const byteStr = atob(b64);
                const bytes = new Uint8Array(byteStr.length);
                for (let i = 0; i < byteStr.length; i++) bytes[i] = byteStr.charCodeAt(i);
                const file = new File([bytes], 'frame.jpg', { type: 'image/jpeg' });

                const result = await html5QrCode.scanFileV2(file, /* showImage */ false);
                if (result?.decodedText) {
                    dispatch('scan-success', { text: result.decodedText });
                    open = false;
                }
            } catch {
                // No QR code in this frame — normal, ignore.
            } finally {
                decoding = false;
            }
        });

        const ulError = tauriListen('qr://error', (msg: string) => {
            error = msg;
        });

        tauriUnlisten = () => { ulFrame(); ulError(); };

        try {
            await tauriBridge.startQrScan();
        } catch (e: any) {
            const msg: string = e?.message ?? String(e);
            // On Android nokhwa is not available; the Rust command returns
            // "use-web-camera" to tell us to fall back to getUserMedia.
            if (msg === 'use-web-camera') {
                tauriUnlisten();
                tauriUnlisten = null;
                html5QrCode = null;
                useWebFallback = true;
                await startWebScan();
                return;
            }
            error = msg;
            active = false;
        }
    }

    // ── Web path ───────────────────────────────────────────────────────────────

    async function startWebScan() {
        await new Promise(r => setTimeout(r, 300));
        if (!document.getElementById(scannerId)) { active = false; return; }

        try {
            const { Html5Qrcode } = await import('html5-qrcode');
            html5QrCode = new Html5Qrcode(scannerId);
            const config = { fps: 10, qrbox: { width: 250, height: 250 } };
            const onSuccess = (text: string) => {
                dispatch('scan-success', { text });
                open = false;
            };
            try {
                await html5QrCode.start({ facingMode: 'environment' }, config, onSuccess, () => {});
            } catch {
                const devices = await Html5Qrcode.getCameras();
                if (!devices.length) throw new Error('No camera found');
                await html5QrCode.start(devices[0].id, config, onSuccess, () => {});
            }
        } catch (e: any) {
            error = e?.message ?? String(e);
            active = false;
        }
    }

    // ── Shared stop ────────────────────────────────────────────────────────────

    async function doStop() {
        active = false;
        decoding = false;
        imgSrc = '';
        error = null;
        useWebFallback = false;
        tauriUnlisten?.();
        tauriUnlisten = null;
        if (tauriBridge.isTauri()) {
            try { await tauriBridge.stopQrScan(); } catch { /* ignore */ }
        } else if (html5QrCode?.isScanning) {
            try { await html5QrCode.stop(); } catch { /* ignore */ }
        }
        html5QrCode = null;
    }

    onDestroy(doStop);
</script>

<Dialog
    bind:open
    aria-labelledby="scanner-title"
    aria-describedby="scanner-content"
    class="qr-scanner-dialog"
>
    <Title id="scanner-title">{$_('qrScanner.title')}</Title>
    <Content id="scanner-content">
        <!-- Hidden canvas used by html5-qrcode's scanFileV2 on the native path -->
        <div id="qr-decode-canvas" style="display:none;"></div>

        {#if tauriBridge.isTauri() && imgSrc}
            <!-- Native camera preview (desktop) -->
            <div class="scanner-body">
                <img src={imgSrc} alt="camera preview" class="scanner-preview" />
            </div>
        {:else if tauriBridge.isTauri() && active && !error && !useWebFallback}
            <div class="scanner-body">
                <CircularProgress class="scanner-progress" indeterminate />
                <p class="scanner-hint">{$_('qrScanner.hint')}</p>
            </div>
        {:else}
            <!-- Web camera path: html5-qrcode mounts into this div -->
            <div id={scannerId} class="scanner-web"></div>
        {/if}

        {#if error}
            <p class="scanner-error">{error}</p>
        {/if}
    </Content>
    <Actions>
        <Button action="close">
            <Label>{$_('qrScanner.cancel')}</Label>
        </Button>
    </Actions>
</Dialog>

<style>
    :global(.qr-scanner-dialog .mdc-dialog__surface) {
        max-width: 500px;
        width: 90vw;
        border-radius: 16px;
    }
    .scanner-body {
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 12px;
        min-height: 280px;
        justify-content: center;
    }
    .scanner-preview {
        width: 100%;
        border-radius: 8px;
        display: block;
    }
    .scanner-web {
        width: 100%;
        min-height: 300px;
        background: #000;
        border-radius: 12px;
        overflow: hidden;
    }
    :global(.scanner-progress) {
        width: 48px !important;
        height: 48px !important;
    }
    .scanner-hint {
        margin: 0;
        color: var(--mdc-theme-text-secondary-on-surface);
        text-align: center;
        font-size: 0.9rem;
    }
    .scanner-error {
        margin: 0;
        color: var(--mdc-theme-error, #d32f2f);
        text-align: center;
        font-size: 0.9rem;
    }
</style>
