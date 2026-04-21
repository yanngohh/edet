<script lang="ts">
    import { onMount } from 'svelte';
    import QRCode from 'qrcode';
    import Dialog, { Content, Title, Actions } from '@smui/dialog';
    import Button, { Label } from '@smui/button';
    import { _ } from 'svelte-i18n';

    export let text: string;
    export let open = false;

    let canvas: HTMLCanvasElement;

    $: if (open && canvas && text) {
        generateQR();
    }

    async function generateQR() {
        try {
            await QRCode.toCanvas(canvas, text, {
                width: 300,
                margin: 2,
                color: {
                    dark: '#000000',
                    light: '#ffffff'
                }
            });
        } catch (err) {
            console.error('Error generating QR code:', err);
        }
    }
</script>

<Dialog
    bind:open
    aria-labelledby="qr-title"
    aria-describedby="qr-content"
    class="qr-dialog"
>
    <Title id="qr-title">{$_("qrCodeDisplay.title")}</Title>
    <Content id="qr-content" class="flex-column align-vcenter">
        <div class="qr-container">
            <canvas bind:this={canvas}></canvas>
        </div>
        <p class="address-text">{text}</p>
    </Content>
    <Actions>
        <Button action="close">
            <Label>{$_("qrCodeDisplay.close")}</Label>
        </Button>
    </Actions>
</Dialog>

<style>
    :global(.qr-dialog .mdc-dialog__surface) {
        max-width: 400px;
        border-radius: 16px;
    }

    .qr-container {
        padding: 16px;
        background: white;
        border-radius: 12px;
        box-shadow: 0 4px 12px rgba(0,0,0,0.1);
        margin-bottom: 16px;
    }

    .address-text {
        font-family: monospace;
        font-size: 0.85rem;
        word-break: break-all;
        text-align: center;
        color: var(--mdc-theme-text-secondary-on-surface);
        background: var(--mdc-theme-background);
        padding: 8px;
        border-radius: 4px;
        margin: 0;
    }
</style>
