<script>
  // @ts-nocheck
  import { onMount } from 'svelte';
  // @ts-ignore
  import renderIdenticon from '@holo-host/identicon';
  import { decodeHashFromBase64 } from '@holochain/client';

  export let agentPubKey; // Base64 encoded public key
  export let size = 32;

  let canvas;

  $: if (canvas && agentPubKey) {
    render();
  }

  function render() {
    if (canvas && agentPubKey) {
      try {
        const hashBytes = decodeHashFromBase64(agentPubKey);
        renderIdenticon({
          hash: hashBytes,
          size
        }, canvas);
      } catch (e) {
        console.error("Error rendering identicon:", e);
      }
    }
  }

  onMount(() => {
    render();
  });
</script>

<span class="agent-avatar" style="width: {size}px; height: {size}px;" role="img"
      aria-label={agentPubKey ? `Agent avatar for ${agentPubKey.slice(0, 8)}…` : 'Agent avatar'}>
  <canvas
    bind:this={canvas}
    width={size}
    height={size}
    aria-hidden="true"
  ></canvas>
</span>

<style>
  .agent-avatar {
    border-radius: 50%;
    overflow: hidden;
    background-color: #eee;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    vertical-align: middle;
    flex-shrink: 0;
  }
</style>
