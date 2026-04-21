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

<div class="agent-avatar" style="width: {size}px; height: {size}px;">
  <!-- role="img" and aria-label give screen readers a meaningful description of the
       identicon avatar.  The label uses the abbreviated public key so it is unique
       and pronounceable without exposing the full 64-character base64 string. -->
  <canvas
    bind:this={canvas}
    width={size}
    height={size}
    role="img"
    aria-label={agentPubKey ? `Agent avatar for ${agentPubKey.slice(0, 8)}…` : 'Agent avatar'}
  ></canvas>
</div>

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
