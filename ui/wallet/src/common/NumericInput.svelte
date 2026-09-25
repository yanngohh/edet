<script lang="ts">
    /**
     * The one numeric input: locale-aware text entry with hold-to-repeat
     * stepper arrows. Two shapes, one control:
     *   - decimal (default): amounts/weights — locale separators, `decimals`
     *     places, cent-sized steps that accelerate;
     *   - integer: epochs/quorums/thresholds — plain digits, unit steps.
     * `min` floors both typing (on blur) and stepping.
     */
    import { createEventDispatcher } from 'svelte';
    import Textfield from "@smui/textfield";
    import { cleanNumberInput, parseNumber, formatNumber } from "./functions";

    export let value: string;
    export let label: string;
    export let invalid: boolean = false;
    export let disabled: boolean = false;
    export let style: string = "";
    /** Whole-number mode (epochs, counts): unit steps, no separators. */
    export let integer: boolean = false;
    /** Lower bound enforced by the steppers and on blur. */
    export let min: number = 0;
    /** Decimal places in decimal mode. */
    export let decimals: number = 2;

    const dispatch = createEventDispatcher();

    let timeoutId: any;
    let intervalId: any;
    let speed = 200; // Initial speed in ms
    let step = integer ? 1 : 0.01;
    let rampUpCount = 0;

    function fmt(val: number): string {
        return integer ? String(Math.round(val)) : formatNumber(val, decimals);
    }

    function parse(input: string): number {
        return integer ? parseInt(input.replace(/[^0-9-]/g, ''), 10) : parseNumber(input);
    }

    function startChanging(direction: 1 | -1) {
        if (disabled) return;
        stopChanging();
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
            speed = Math.max(20, speed * 0.8);
            if (integer) {
                if (rampUpCount > 25) step = 5;
                if (rampUpCount > 50) step = 10;
            } else {
                if (rampUpCount > 20) step = 0.1;
                if (rampUpCount > 40) step = 1.0;
                if (rampUpCount > 60) step = 10.0;
            }
        }

        intervalId = setTimeout(() => {
            continueChanging(direction);
        }, speed);
    }

    function stopChanging() {
        if (timeoutId) clearTimeout(timeoutId);
        if (intervalId) clearTimeout(intervalId);
        speed = 200;
        step = integer ? 1 : 0.01;
        rampUpCount = 0;
    }

    function changeValue(direction: 1 | -1) {
        let val = parse(value);
        if (isNaN(val)) val = min;
        val = Math.max(min, val + step * direction);
        value = fmt(val);
        dispatch('input');
    }

    function onBlur() {
        const cleaned = integer ? value.replace(/[^0-9]/g, '') : cleanNumberInput(value);
        const parsed = parse(cleaned);
        if (!isNaN(parsed) && isFinite(parsed)) {
            value = fmt(Math.max(min, parsed));
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
               input$inputmode={integer ? "numeric" : "decimal"}
               {invalid}
               {disabled}
               bind:value={value}
               on:blur={onBlur}
               on:input>
    </Textfield>

    <div class="stepper-controls" class:disabled>
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

    .stepper-controls.disabled .stepper-btn {
        opacity: 0.38;
        cursor: default;
        pointer-events: none;
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
