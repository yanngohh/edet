<script lang="ts">
    import { createEventDispatcher } from 'svelte';
    import Textfield from "@smui/textfield";
    import IconButton from '@smui/icon-button';
    import { cleanNumberInput, parseNumber, formatNumber } from "./functions";

    export let value: string;
    export let label: string;
    export let invalid: boolean = false;
    export let style: string = "";

    const dispatch = createEventDispatcher();

    let timeoutId: any;
    let intervalId: any;
    let speed = 200; // Initial speed in ms
    let step = 0.01; // Initial step (few cents)
    let rampUpCount = 0;

    function startChanging(direction: 1 | -1) {
        stopChanging(); // clear any existing
        changeValue(direction);
        
        // Initial delay before continuous change
        timeoutId = setTimeout(() => {
            continueChanging(direction);
        }, 500);
    }

    function continueChanging(direction: 1 | -1) {
        changeValue(direction);
        
        // Acceleration logic
        rampUpCount++;
        if (rampUpCount > 5) {
            speed = Math.max(20, speed * 0.8); // Speed up
            if (rampUpCount > 20) step = 0.1; // Larger chunks
            if (rampUpCount > 40) step = 1.0; 
            if (rampUpCount > 60) step = 10.0;
        }

        intervalId = setTimeout(() => {
            continueChanging(direction);
        }, speed);
    }

    function stopChanging() {
        if (timeoutId) clearTimeout(timeoutId);
        if (intervalId) clearTimeout(intervalId);
        speed = 200;
        step = 0.01;
        rampUpCount = 0;
    }

    function changeValue(direction: 1 | -1) {
        let val = parseNumber(value);
        if (isNaN(val)) val = 0;
        val = Math.max(0, val + (step * direction));
        value = formatNumber(val, 2);
        dispatch('input');
    }

    function onBlur() {
        const cleaned = cleanNumberInput(value);
        const parsed = parseNumber(cleaned);
        if (!isNaN(parsed) && parsed > 0) {
            value = formatNumber(parsed, 2);
        } else {
            value = cleaned;
        }
        dispatch('blur');
    }
</script>

<div class="numeric-input-container" {style}>
    <Textfield class="tf-numeric"
               {label}
               style="width: 100%; flex: 1;"
               type="text"
               input$inputmode="decimal"
               {invalid}
               bind:value={value}
               on:blur={onBlur}
               on:input>
    </Textfield>

    <div class="stepper-controls">
        <div class="stepper-btn up"
             role="button"
             tabindex="0"
             aria-label="Increase value"
             on:mousedown|preventDefault={() => startChanging(1)}
             on:touchstart|preventDefault={() => startChanging(1)}
             on:mouseup={stopChanging}
             on:mouseleave={stopChanging}
             on:touchend={stopChanging}
             on:keydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); changeValue(1); } }}
        >
            <i class="material-icons">keyboard_arrow_up</i>
        </div>
        <div class="stepper-btn down"
             role="button"
             tabindex="0"
             aria-label="Decrease value"
             on:mousedown|preventDefault={() => startChanging(-1)}
             on:touchstart|preventDefault={() => startChanging(-1)}
             on:mouseup={stopChanging}
             on:mouseleave={stopChanging}
             on:touchend={stopChanging}
             on:keydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); changeValue(-1); } }}
        >
            <i class="material-icons">keyboard_arrow_down</i>
        </div>
    </div>
</div>

<style>
    .numeric-input-container {
        display: flex;
        flex-direction: row;
        align-items: center;
        position: relative;
    }

    .stepper-controls {
        display: flex;
        flex-direction: column;
        width: 32px;
        height: 100%;
        margin-left: 4px;
        justify-content: center;
        gap: 2px;
    }

    .stepper-btn {
        display: flex;
        align-items: center;
        justify-content: center;
        background: rgba(0,0,0,0.05);
        border-radius: 4px;
        cursor: pointer;
        height: 24px;
        color: var(--mdc-theme-primary);
        user-select: none;
        transition: background 0.2s;
    }

    .stepper-btn:hover {
        background: rgba(0,0,0,0.1);
    }
    
    .stepper-btn:active {
        background: var(--mdc-theme-primary);
        color: white;
    }

    :global(.dark-theme) .stepper-btn {
        background: rgba(255,255,255,0.1);
    }
    :global(.dark-theme) .stepper-btn:hover {
        background: rgba(255,255,255,0.2);
    }

    .material-icons {
        font-size: 20px;
    }
</style>
