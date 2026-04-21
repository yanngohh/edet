<script lang="ts">
    import { onMount, onDestroy, createEventDispatcher } from 'svelte';
    import { Html5Qrcode } from 'html5-qrcode';
    import Dialog, { Content, Title, Actions } from '@smui/dialog';
    import Button, { Label } from '@smui/button';
    import { _ } from 'svelte-i18n';

    export let open = false;

    const dispatch = createEventDispatcher();
    let html5QrCode: Html5Qrcode | null = null;
    const scannerId = "qr-reader";

    $: if (open) {
        startScanner();
    } else {
        stopScanner();
    }

    async function startScanner() {
        // Wait for the dialog to be in the DOM
        setTimeout(async () => {
            const element = document.getElementById(scannerId);
            if (!element) return;

            try {
                if (!html5QrCode) {
                    html5QrCode = new Html5Qrcode(scannerId);
                }

                const config = { fps: 10, qrbox: { width: 250, height: 250 } };
                
                await html5QrCode.start(
                    { facingMode: "environment" },
                    config,
                    (decodedText) => {
                        dispatch('scan-success', { text: decodedText });
                        open = false;
                    },
                    (errorMessage) => {
                        // ignore failures to find a QR code in a frame
                    }
                );
            } catch (err) {
                console.error("Unable to start scanning", err);
            }
        }, 300);
    }

    async function stopScanner() {
        if (html5QrCode && html5QrCode.isScanning) {
            try {
                await html5QrCode.stop();
            } catch (err) {
                console.error("Unable to stop scanning", err);
            }
        }
    }

    onDestroy(() => {
        stopScanner();
    });
</script>

<Dialog
    bind:open
    aria-labelledby="scanner-title"
    aria-describedby="scanner-content"
    class="qr-scanner-dialog"
>
    <Title id="scanner-title">{$_("qrScanner.title")}</Title>
    <Content id="scanner-content">
        <div id={scannerId} style="width: 100%; min-height: 300px; background: #000; border-radius: 12px; overflow: hidden;"></div>
    </Content>
    <Actions>
        <Button action="close">
            <Label>{$_("qrScanner.cancel")}</Label>
        </Button>
    </Actions>
</Dialog>

<style>
    :global(.qr-scanner-dialog .mdc-dialog__surface) {
        max-width: 500px;
        width: 90vw;
        border-radius: 16px;
    }
</style>
