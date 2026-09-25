<script>
  import { onMount, createEventDispatcher } from "svelte";
  import { hint } from "./hint.js";
  import { money } from "./load.js";
  import { shortName, dayOfTick } from "./analytics.js";
  import { orbitRested, orbitWatch, satellitesOf } from "./network.js";
  import Icon from "./Icon.svelte";
  

  export let day;
  export let run;
  export let positions;
  export let scales;
  export let selected = null;
  export let reducedMotion = false;
  const dispatch = createEventDispatcher();
  let canvas, container;
  let width = 800,
    height = 480;
  let mode = "3d",
    stakes = true,
    contracts = false,
    // The scene turns on its own until the machine says it cannot afford to.
    orbit = true,
    autoStopped = false;
  let watch = orbitRested();
  // **Looking down on the community, not along it.** `tilt` pitches the
  // camera: near zero the scene is edge-on and a graph of stakes reads as a
  // line of dots, and the sign decides which side of it you stand on —
  // positive puts the far side of the scene higher on the screen, which is
  // what looking down at something means. 0.785 is 45 degrees.
  let rotation = -0.3,
    tilt = 0.785,
    zoom = 1;
  let hovered = null,
    // A person without an account under the pointer, by person index: they
    // have no member id, so they are not `hovered`.
    hoveredSat = null,
    pointer = { x: 0, y: 0 },
    drag = null,
    moved = false;
  let projected = [],
    projectedSat = [],
    frame,
    ctx,
    dirty = true;
  let visible = true;
  // The canvas cannot read a CSS variable, so the palette is read off the
  // document and kept in step with the theme: orange for a member on black,
  // purple on white, with the second accent for an underwriter.
  let colors = { member: [255, 106, 51], underwriter: [180, 120, 216], default: [255, 93, 108], inactive: [128, 128, 128] };
  const cssRgb = (name, fallback) => {
    if (typeof getComputedStyle === "undefined") return fallback;
    const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
    const hex = v.match(/^#([0-9a-f]{6})$/i);
    if (hex) return [0, 2, 4].map((i) => parseInt(hex[1].slice(i, i + 2), 16));
    const rgb = v.match(/(\d+)\D+(\d+)\D+(\d+)/);
    return rgb ? [+rgb[1], +rgb[2], +rgb[3]] : fallback;
  };
  let panel = "#141216";
  function readPalette() {
    panel = getComputedStyle(document.documentElement).getPropertyValue("--panel").trim() || panel;
    colors = {
      member: cssRgb("--mint", colors.member),
      underwriter: cssRgb("--lavender", colors.underwriter),
      default: cssRgb("--coral", colors.default),
      inactive: cssRgb("--muted", colors.inactive),
    };
  }
  $: if (theme !== undefined) {
    readPalette();
    // The palette is read into `colors`, which the invalidation block below
    // does not watch: without this the canvas kept the old theme's colours
    // until something else happened to make it redraw.
    dirty = true;
  }
  export let theme = "dark";

  // Whether this frame is one of a sequence: what moves may be drawn moving.
  let animating = false;
  const rgba = (rgb, alpha) => `rgba(${rgb.join(",")},${alpha})`;
  $: maxCapacity = scales.capacity;
  $: maxStake = scales.stake;
  $: if (day) {
    hovered = null;
    hoveredSat = null;
  }
  // **The people the ledger has no row for are still in the picture.** They
  // are drawn as faint satellites of whoever introduced them, or on an outer
  // ring when nobody did, so a town of 128 with 15 members looks like one.
  $: satellites = satellitesOf(run, day, positions);
  $: waiting = satellites.length;
  $: {
    day;
    selected;
    positions;
    scales;
    reducedMotion;
    mode;
    stakes;
    contracts;
    zoom;
    rotation;
    tilt;
    hovered;
    hoveredSat;
    dirty = true;
  }
  $: hoverMember = hovered === null ? null : day.members.find((m) => m.id === hovered);
  $: hoverPerson = hoveredSat === null ? null : run.persons[hoveredSat];
  $: hoverPurse = hoveredSat === null ? null : (day.purses || []).find((x) => x.person === hoveredSat);

  function project(point) {
    const cosine = Math.cos(rotation),
      sine = Math.sin(rotation);
    const flat = mode === "2d";
    const x = flat ? (point.fx ?? point.x) : point.x * cosine - point.z * sine;
    const z0 = flat ? 0 : point.x * sine + point.z * cosine;
    const y = flat ? (point.fy ?? point.y) : point.y * Math.cos(tilt) - z0 * Math.sin(tilt);
    const z = mode === "2d" ? 0 : point.y * Math.sin(tilt) + z0 * Math.cos(tilt);
    const perspective = 780 / (780 + z);
    const scale = Math.min(width / 740, height / 420) * zoom;
    return {
      x: width / 2 + x * scale * perspective,
      y: height * 0.47 + y * scale * perspective,
      z,
      scale: scale * perspective,
    };
  }

  function line(a, b, color, alpha, lineWidth, dashed = false, directed = false) {
    ctx.beginPath();
    ctx.moveTo(a.x, a.y);
    ctx.lineTo(b.x, b.y);
    ctx.strokeStyle = rgba(color, alpha);
    ctx.lineWidth = lineWidth;
    ctx.setLineDash(dashed ? [4, 5] : []);
    ctx.stroke();
    ctx.setLineDash([]);
    if (directed)
      chevron(
        a.x + (b.x - a.x) * 0.71,
        a.y + (b.y - a.y) * 0.71,
        Math.atan2(b.y - a.y, b.x - a.x),
        color,
        Math.min(1, alpha * 1.6),
      );
  }

  /** An arrowhead pointing the way an arc runs. It is drawn either standing
   *  still on the arc, or travelling along it — never both, because two
   *  arrowheads on one line read as two backings. */
  function chevron(x, y, angle, color, alpha, size = 4) {
    ctx.beginPath();
    ctx.moveTo(x - size * Math.cos(angle - 0.5), y - size * Math.sin(angle - 0.5));
    ctx.lineTo(x, y);
    ctx.lineTo(x - size * Math.cos(angle + 0.5), y - size * Math.sin(angle + 0.5));
    ctx.strokeStyle = rgba(color, alpha);
    ctx.lineWidth = 1.3;
    ctx.lineJoin = "round";
    ctx.stroke();
  }

  function draw(time) {
    if (!ctx || !width || !height) return;
    ctx.clearRect(0, 0, width, height);
    const halo = ctx.createRadialGradient(
      width * 0.5,
      height * 0.46,
      10,
      width * 0.5,
      height * 0.46,
      width * 0.44,
    );
    halo.addColorStop(0, rgba(colors.member, 0.05));
    halo.addColorStop(1, rgba(colors.member, 0));
    ctx.fillStyle = halo;
    ctx.fillRect(0, 0, width, height);
    // A perspective floor anchors the spatial view; it carries no data.
    if (mode === "3d")
      for (const radius of [100, 170, 240, 300]) {
        ctx.beginPath();
        for (let i = 0; i <= 100; i++) {
          const angle = (i / 100) * Math.PI * 2;
          const p = project({ x: Math.cos(angle) * radius, y: 220, z: Math.sin(angle) * radius });
          if (i === 0) ctx.moveTo(p.x, p.y);
          else ctx.lineTo(p.x, p.y);
        }
        ctx.strokeStyle = rgba(colors.inactive, 0.07);
        ctx.lineWidth = 1;
        ctx.stroke();
      }
    projected = day.members
      .map((m) => ({ ...project(positions.get(m.id) || { x: 0, y: 0, z: 0 }), m }))
      .sort((a, b) => b.z - a.z);
    const nodes = new Map(projected.map((p) => [p.m.id, p]));
    const selectedMember = day.members.find((m) => m.person === selected && selected !== null);
    const focus = hovered ?? selectedMember?.id;
    // The satellites go under everything: faint, small, and tethered to the
    // sponsor by a hairline. None of it is a measured quantity.
    projectedSat = satellites.map((s) => ({ ...project(s), sat: s })).sort((a, b) => b.z - a.z);
    for (const q of projectedSat) {
      const anchor = q.sat.sponsor == null ? null : nodes.get(q.sat.sponsor);
      const lit = hoveredSat === q.sat.person || selected === q.sat.person;
      const near = focus == null || anchor?.m.id === focus;
      const alpha = lit ? 0.9 : near ? 0.38 : 0.1;
      if (anchor) line(anchor, q, colors.inactive, alpha * 0.45, 0.6);
      const radius = 2.6 * q.scale;
      q.radius = radius;
      ctx.fillStyle = rgba(colors.inactive, alpha);
      ctx.beginPath();
      ctx.arc(q.x, q.y, radius, 0, Math.PI * 2);
      ctx.fill();
      if (lit) {
        ctx.beginPath();
        ctx.arc(q.x, q.y, radius + 5, 0, Math.PI * 2);
        ctx.strokeStyle = rgba(colors.inactive, 0.9);
        ctx.lineWidth = 1.2;
        ctx.stroke();
      }
    }
    const neighbors = new Set(focus == null ? [] : [focus]);
    if (focus != null) {
      if (stakes)
        for (const [a, b] of day.edges) {
          if (a === focus) neighbors.add(b);
          if (b === focus) neighbors.add(a);
        }
      if (contracts)
        for (const c of day.contracts.filter(
          (c) => c.status === "active" || c.status === "expired",
        )) {
          if (c.creditor === focus) neighbors.add(c.debtor);
          if (c.debtor === focus) neighbors.add(c.creditor);
        }
    }
    if (contracts)
      for (const c of day.contracts) {
        const a = nodes.get(c.creditor),
          b = nodes.get(c.debtor);
        if (!a || !b || !["active", "expired"].includes(c.status)) continue;
        const relevant = focus == null || c.creditor === focus || c.debtor === focus;
        line(
          a,
          b,
          c.status === "expired" ? colors.default : colors.underwriter,
          relevant ? 0.43 : 0.045,
          1,
          !c.insured,
        );
      }
    if (stakes)
      day.edges.forEach(([from, to, weight], i) => {
        const a = nodes.get(from),
          b = nodes.get(to);
        if (!a || !b) return;
        const relevant = focus == null || from === focus || to === focus;
        const alpha = relevant ? (focus == null ? 0.16 + (0.22 * weight) / maxStake : 0.7) : 0.045;
        // The direction is said ONCE: by an arrowhead running the arc while
        // the scene animates, and by one standing at 71% of it while the scene
        // is still. A runner frozen mid-arc would be a static arrow in an
        // arbitrary place, which is worse than the one that means to be there.
        const running = animating && relevant && !reducedMotion && day.edges.length < 450;
        line(a, b, colors.member, alpha, 0.6 + (1.6 * weight) / maxStake, false, !running);
        if (running) {
          const t = (time / 7500 + i * 0.173) % 1;
          chevron(
            a.x + (b.x - a.x) * t,
            a.y + (b.y - a.y) * t,
            Math.atan2(b.y - a.y, b.x - a.x),
            colors.member,
            focus == null ? 0.75 : 0.95,
            4.5,
          );
        }
      });
    const labels = [];
    for (const p of projected) {
      const member = p.m;
      const color =
        member.supply > 0
          ? colors.underwriter
          : member.status === "active"
            ? colors.member
            : colors.inactive;
      const radius = (4 + 9 * Math.sqrt(member.capacity / maxCapacity)) * p.scale;
      p.radius = radius;
      const focused = member.id === focus;
      const alpha = focus == null || neighbors.has(member.id) ? 1 : 0.18;
      // **The flat view is drawn flat.** Depth shading in a view with no depth
      // is decoration that reads as data — a lit sphere looks nearer than a
      // dull one, and nothing here is nearer. In 3D the shading IS the depth
      // cue, and there it stays.
      if (mode === "2d") {
        ctx.fillStyle = rgba(color, alpha);
        ctx.beginPath();
        ctx.arc(p.x, p.y, radius, 0, Math.PI * 2);
        ctx.fill();
      } else {
        const glow = ctx.createRadialGradient(p.x, p.y, 0, p.x, p.y, radius * 3.8);
        glow.addColorStop(0, rgba(color, 0.19 * alpha));
        glow.addColorStop(1, rgba(color, 0));
        ctx.fillStyle = glow;
        ctx.beginPath();
        ctx.arc(p.x, p.y, radius * 3.8, 0, Math.PI * 2);
        ctx.fill();
        const sphere = ctx.createRadialGradient(
          p.x - radius * 0.35,
          p.y - radius * 0.4,
          0,
          p.x,
          p.y,
          radius * 1.2,
        );
        sphere.addColorStop(
          0,
          rgba(
            color.map((c) => Math.min(255, c + 50)),
            alpha,
          ),
        );
        sphere.addColorStop(0.35, rgba(color, alpha));
        sphere.addColorStop(
          1,
          rgba(
            color.map((c) => Math.round(c * 0.25)),
            alpha,
          ),
        );
        ctx.fillStyle = sphere;
        ctx.beginPath();
        ctx.arc(p.x, p.y, radius, 0, Math.PI * 2);
        ctx.fill();
      }
      if (member.open_default > 0 || focused) {
        ctx.beginPath();
        ctx.arc(p.x, p.y, radius + 5, 0, Math.PI * 2);
        ctx.strokeStyle = rgba(member.open_default > 0 ? colors.default : color, alpha * 0.9);
        ctx.lineWidth = 1.4;
        ctx.stroke();
        if (focused) {
          ctx.beginPath();
          ctx.arc(p.x, p.y, radius + 10, 0, Math.PI * 2);
          ctx.strokeStyle = rgba(color, 0.2);
          ctx.stroke();
        }
      }
      if (
        focused ||
        (focus == null && (member.supply > 0 || member.open_default > 0 || member.id % 9 === 0))
      ) {
        labels.push({
          p,
          radius,
          focused,
          priority: focused ? 3 : member.open_default > 0 ? 2 : member.supply > 0 ? 1 : 0,
        });
      }
    }
    const placed = [];
    for (const { p, radius, focused } of labels.sort((a, b) => b.priority - a.priority)) {
      const label = shortName(run, p.m.person);
      ctx.font = `${focused ? 600 : 400} 11px Inter, system-ui, sans-serif`;
      ctx.textAlign = "center";
      const labelWidth = ctx.measureText(label).width;
      for (const labelY of [p.y + radius + 19, p.y - radius - 12, p.y + radius + 34]) {
        const box = { x: p.x - labelWidth / 2 - 5, y: labelY - 11, w: labelWidth + 10, h: 17 };
        if (box.x < 8 || box.x + box.w > width - 8 || box.y < 42 || box.y + box.h > height - 45)
          continue;
        if (
          placed.some(
            (b) =>
              box.x < b.x + b.w + 4 &&
              box.x + box.w + 4 > b.x &&
              box.y < b.y + b.h + 2 &&
              box.y + box.h + 2 > b.y,
          )
        )
          continue;
        placed.push(box);
        ctx.fillStyle = panel;
        ctx.fillRect(box.x, box.y, box.w, box.h);
        ctx.fillStyle = rgba(focused ? colors.member : colors.inactive, 1);
        ctx.fillText(label, p.x, labelY);
        break;
      }
    }
    if (!projected.length) {
      ctx.fillStyle = rgba(colors.inactive, 1);
      ctx.textAlign = "center";
      ctx.font = "14px system-ui";
      ctx.fillText("No members on this day", width / 2, height / 2);
    }
    dirty = false;
  }

  function locate(e) {
    const rect = canvas.getBoundingClientRect();
    pointer = { x: e.clientX - rect.left, y: e.clientY - rect.top };
    if (drag) {
      const dx = e.clientX - drag.x,
        dy = e.clientY - drag.y;
      if (Math.hypot(dx, dy) > 3) moved = true;
      if (mode === "3d") {
        rotation += dx * 0.006;
        tilt = Math.max(-1.1, Math.min(1.1, tilt + dy * 0.004));
      }
      drag = { x: e.clientX, y: e.clientY };
      dirty = true;
      return;
    }
    hovered =
      [...projected]
        .reverse()
        .find((p) => Math.hypot(p.x - pointer.x, p.y - pointer.y) <= Math.max(12, p.radius + 5))?.m
        .id ?? null;
    // A member under the pointer wins; a satellite is found only where none is.
    hoveredSat =
      hovered !== null
        ? null
        : [...projectedSat]
            .reverse()
            .find((q) => Math.hypot(q.x - pointer.x, q.y - pointer.y) <= Math.max(9, q.radius + 4))?.sat
            .person ?? null;
  }
  function startDrag(e) {
    if (e.button !== 0) return;
    locate(e);
    drag = { x: e.clientX, y: e.clientY };
    moved = false;
    canvas.setPointerCapture(e.pointerId);
  }
  function stopDrag(e) {
    if (!drag) return;
    if (!moved) {
      const member = day.members.find((m) => m.id === hovered);
      if (member?.person != null) dispatch("choose", member.person);
      else if (hoveredSat !== null) dispatch("choose", hoveredSat);
    } else {
      // **Only a drag claims the arrows.** Focusing the canvas on every
      // pointerdown meant one click on a node handed ← and → to the camera for
      // the rest of the session, while the footer still said they stepped the
      // day and nothing on screen said otherwise. A reader who dragged the
      // scene is steering it and keeps them; tabbing to the canvas still works.
      container.focus({ preventScroll: true });
    }
    drag = null;
    if (canvas.hasPointerCapture(e.pointerId)) canvas.releasePointerCapture(e.pointerId);
  }
  function reset() {
    rotation = -0.3;
    tilt = 0.785;
    zoom = 1;
    // Back to how the scene opens, which is turning — unless this machine has
    // already said it cannot afford to.
    orbit = !autoStopped;
    watch = orbitRested();
  }
  function wheel(e) {
    const next = Math.max(0.55, Math.min(2.3, zoom - e.deltaY * 0.001));
    // At either end of the zoom the wheel has nothing left to do, so the page
    // scrolls instead. The canvas is 440px of a long page and swallowing every
    // wheel event over it left a reader with no way past but to aim elsewhere.
    if (next === zoom) return;
    e.preventDefault();
    zoom = next;
  }
  function keyboard(e) {
    if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "+", "-", "Home"].includes(e.key))
      e.preventDefault();
    if (e.key === "ArrowLeft") rotation -= 0.1;
    if (e.key === "ArrowRight") rotation += 0.1;
    if (e.key === "ArrowUp") tilt = Math.max(-1.1, tilt - 0.1);
    if (e.key === "ArrowDown") tilt = Math.min(1.1, tilt + 0.1);
    if (e.key === "+") zoom = Math.min(2.3, zoom + 0.1);
    if (e.key === "-") zoom = Math.max(0.55, zoom - 0.1);
    if (e.key === "Home") reset();
  }
  onMount(() => {
    ctx = canvas.getContext("2d");
    const resize = new ResizeObserver(([entry]) => {
      width = entry.contentRect.width;
      height = entry.contentRect.height;
      // A run of fifty people over two hundred days redraws this a great many
      // times: a display's full pixel ratio buys nothing here and costs four
      // times the fill.
      const ratio = Math.min(window.devicePixelRatio || 1, 1.5);
      canvas.width = Math.round(width * ratio);
      canvas.height = Math.round(height * ratio);
      ctx?.setTransform(ratio, 0, 0, ratio, 0, 0);
      draw(performance.now());
    });
    resize.observe(container);
    const intersection = new IntersectionObserver(([entry]) => {
      visible = entry.isIntersecting;
    });
    intersection.observe(container);
    let last = 0;
    // **Draw when something changed, not because time passed.** The orbit is
    // the only thing that moves by itself, and it stops while a person is
    // being read: a still scene costs nothing then, which is what lets a big
    // run stay responsive while somebody scrubs the days.
    const animate = (time) => {
      const turning =
        orbit && !reducedMotion && !drag && mode === "3d" && selected == null && hovered == null && hoveredSat == null;
      // The dots that show a stake's direction are themselves motion, and
      // they are exactly what somebody is watching when they hover or select
      // a person: a scene that stops drawing then is a scene that lies about
      // which way a backing runs. Draw while anything is moving; rest only
      // when nothing is.
      // Only when something can actually move: above the edge bound no chevron
      // runs, and a scene where nothing moves was still being redrawn 25 times
      // a second for as long as a person was selected.
      const flowing =
        !reducedMotion && stakes && day.edges.length < 450 && (hovered != null || selected != null);
      if (visible && !document.hidden && time - last >= 40) {
        if (turning) rotation += Math.min(time - last, 60) * 0.000055;
        if (dirty || turning || flowing) {
          animating = turning || flowing;
          const began = performance.now();
          draw(time);
          if (turning) {
            watch = orbitWatch(watch, performance.now() - began);
            if (watch.stop) {
              orbit = false;
              autoStopped = true;
            }
          }
          dirty = false;
        }
        last = time;
      }
      frame = requestAnimationFrame(animate);
    };
    frame = requestAnimationFrame(animate);
    return () => {
      resize.disconnect();
      intersection.disconnect();
      cancelAnimationFrame(frame);
    };
  });
</script>

<section class="network-panel panel">
  <div class="network-header">
    <div><div class="eyebrow">THE BIG PICTURE</div><h2>The community <span class="count">{day.members.length}</span>{#if waiting}<span class="waiting-count" use:hint={"People in the town the ledger has no row for: introduced by an offer that was never seated, or never offered a trade. Drawn as faint dots around whoever introduced them."}>+ {waiting} without an account</span>{/if}</h2></div>
    <div class="segmented" aria-label="Network dimension"><button class:active={mode === '3d'} on:click={() => mode = '3d'} aria-pressed={mode === '3d'}>3D view</button><button class:active={mode === '2d'} on:click={() => mode = '2d'} aria-pressed={mode === '2d'}>2D</button></div>
  </div>
  <div class="layer-bar">
    <span class="layer-caption">Show connections</span>
    <button class:enabled={stakes} on:click={() => stakes = !stakes} aria-pressed={stakes}><span class="layer-dot mint"></span>Backings <span class="layer-check">{stakes ? '✓' : '+'}</span></button>
    <button class:enabled={contracts} on:click={() => contracts = !contracts} aria-pressed={contracts}><span class="layer-dot lavender"></span>Contracts <span class="layer-check">{contracts ? '✓' : '+'}</span></button>

  </div>
  <!-- The spatial camera is a custom keyboard-operated application; the People list supplies semantic member controls. -->
  <!-- svelte-ignore a11y-no-noninteractive-tabindex a11y-no-noninteractive-element-interactions -->
  <div class="canvas-container" bind:this={container} role="application" tabindex="0" aria-label="Interactive community network. Drag to rotate, scroll to zoom, or use arrow keys. Select a member using the people list for keyboard access." on:keydown={keyboard}>
    <div class="scene-caption"><span class="crosshair">+</span> COMMUNITY FIELD <span class="scene-divider">/</span> {String(day.epoch).padStart(3, '0')}</div>
    <canvas bind:this={canvas} class:dragging={drag} class:pointing={hovered !== null || hoveredSat !== null} aria-label="Members sized by individual capacity and linked by directed backings" on:pointerdown={startDrag} on:pointermove={locate} on:pointerup={stopDrag} on:pointercancel={() => drag = null} on:pointerleave={() => { if (!drag) hovered = null; }} on:wheel={wheel}></canvas>
    {#if hoverPerson && !drag}
      <div class="network-tooltip" style:left="{Math.max(8, Math.min(pointer.x + 18, width - 222))}px" style:top="{Math.max(8, Math.min(pointer.y + 18, height - 120))}px">
        <strong>{shortName(run, hoveredSat)}</strong><span>No account yet · waiting since day {dayOfTick(run, hoverPerson.joined_tick)}</span>
        {#if hoverPerson.introduced_by != null}<div>Introduced by <b>{shortName(run, hoverPerson.introduced_by)}</b></div>{/if}
        {#if hoverPurse}<div>Cash on hand <b>{money(hoverPurse.cash)}</b></div>{/if}
      </div>
    {/if}
    {#if hoverMember && !drag}
      <div class="network-tooltip" style:left="{Math.max(8, Math.min(pointer.x + 18, width - 222))}px" style:top="{Math.max(8, Math.min(pointer.y + 18, height - 120))}px">
        <strong>{shortName(run, hoverMember.person)}</strong><span>{hoverMember.supply > 0 ? 'Underwriter' : 'Community member'}</span>
        <div>Capacity <b>{money(hoverMember.capacity)}</b></div><div>Outstanding debt <b>{money(hoverMember.debt)}</b></div>
        {#if hoverMember.open_default > 0}<div class="coral-text">In default <b>{money(hoverMember.open_default)}</b></div>{/if}
      </div>
    {/if}
    <div class="scene-instructions"><Icon name="globe" size={14} />{mode === '3d' ? 'Drag to turn' : 'Select a person'}<span>·</span>Scroll to zoom<span>·</span>Click to open a person</div>
    <div class="scene-tools">
      <button class="icon-button" title="Zoom in" aria-label="Zoom in" on:click={() => zoom = Math.min(2.3, zoom + .15)}><Icon name="plus" size={16} /></button>
      <button class="icon-button" title="Zoom out" aria-label="Zoom out" on:click={() => zoom = Math.max(.55, zoom - .15)}><Icon name="minus" size={16} /></button>
      <span></span><button class="icon-button" use:hint={"Reset camera"} aria-label="Reset camera" on:click={reset}><Icon name="reset" size={15} /></button>
      <button class="icon-button" class:active={orbit} title={autoStopped ? 'Auto rotate — stopped itself: this scene costs more to draw than the page can spare' : 'Auto rotate'} aria-label="Auto rotate" aria-pressed={orbit} disabled={reducedMotion || mode === '2d'} on:click={() => { orbit = !orbit; if (orbit) { watch = orbitRested(); autoStopped = false; } }}><Icon name="globe" size={16} /></button>
    </div>
  </div>
  <div class="network-legend"><span><i class="legend-node mint"></i>Member</span><span><i class="legend-node lavender"></i>Underwriter</span><span><i class="legend-node default"></i>In default</span><span><i class="legend-node waiting"></i>No account yet</span>{#if contracts}<span><i class="contract-line"></i>Contract · dashed = uninsured</span>{/if}<span class="legend-scale" use:hint={"Capacity and backing widths use fixed scales across the tape. Perspective affects apparent size; use 2D for comparisons. Zero-capacity nodes keep a minimum radius."}>Node size = individual capacity <Icon name="info" size={13} /></span></div>
  <div class="network-note">A backing runs creditor → debtor, is written only by discharge, and is drawn by what was staked. A node is drawn by the capacity that backing earns them. Contracts are the other relation: what somebody owes. Position is for readability; the arrowhead shows direction.</div>
</section>

<style>
  .network-panel {
    overflow: hidden;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .network-header {
    padding: 23px 25px 16px;
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
  }
  h2 {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 7px;
  }
  .waiting-count {
    color: var(--muted);
    font-size: 11px;
    font-weight: 400;
    cursor: help;
  }
  .count {
    color: var(--muted);
    border: 1px solid var(--line);
    border-radius: 5px;
    padding: 2px 7px;
    font-size: 11px;
    font-weight: 400;
  }
  .layer-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 25px 12px;
  }
  .layer-caption {
    font-size: 11px;
    color: var(--muted);
    margin-right: 4px;
  }
  .layer-bar button {
    display: flex;
    gap: 7px;
    align-items: center;
    font-size: 10px;
    color: var(--muted);
    padding: 5px 8px;
    border: 1px solid var(--line);
    border-radius: 5px;
    background: transparent;
  }
  .layer-bar button.enabled {
    color: var(--text);
    background: var(--panel);
    border-color: var(--line);
  }
  .layer-check {
    margin-left: 8px;
    color: var(--mint);
  }
  .layer-dot {
    height: 5px;
    width: 5px;
    border-radius: 50%;
  }
  .layer-dot.mint { background: var(--mint); }
  .layer-dot.lavender { background: var(--lavender); }

  .canvas-container {
    position: relative;
    flex: 1;
    height: 440px;
    min-height: 390px;
    background-image: radial-gradient(color-mix(in srgb, var(--text) 10%, transparent) 0.7px, transparent 0.7px);
    background-size: 24px 24px;
    border-top: 1px solid var(--line);
    overflow: hidden;
  }
  canvas {
    position: absolute;
    inset: 0;
    display: block;
    width: 100%;
    height: 100%;
    cursor: grab;
    touch-action: none;
  }
  canvas.dragging {
    cursor: grabbing;
  }
  canvas.pointing {
    cursor: pointer;
  }
  .scene-caption {
    position: absolute;
    top: 17px;
    left: 25px;
    font-family: var(--mono);
    font-size: 9px;
    letter-spacing: 1px;
    color: var(--muted);
    pointer-events: none;
  }
  .crosshair {
    color: var(--muted);
    font-size: 14px;
    margin-right: 7px;
  }
  .scene-divider {
    margin: 0 8px;
    color: var(--muted);
  }
  .scene-instructions {
    position: absolute;
    bottom: 22px;
    left: 25px;
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 10px;
    color: var(--secondary);
    pointer-events: none;
  }
  .scene-instructions span {
    color: var(--muted);
  }
  .scene-tools {
    position: absolute;
    right: 20px;
    bottom: 14px;
    display: flex;
    gap: 2px;
    background: color-mix(in srgb, var(--panel) 92%, transparent);
    border: 1px solid var(--line);
    padding: 3px;
    border-radius: 7px;
  }
  .scene-tools > span {
    width: 1px;
    background: var(--line);
    margin: 6px 3px;
  }
  .scene-tools .active {
    color: var(--mint);
    background: var(--line);
  }
  .network-legend {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 17px;
    padding: 15px 25px 10px;
    border-top: 1px solid var(--line);
    font-size: 10px;
    color: var(--secondary);
  }
  .network-legend > span {
    display: flex;
    align-items: center;
    gap: 7px;
  }
  .legend-node {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    display: inline-block;
  }
  .mint {
    background: var(--mint);
  }
  .lavender {
    background: var(--lavender);
  }
  .legend-node.default {
    border: 1.5px solid var(--coral);
    width: 8px;
    height: 8px;
    background: transparent;
  }
  .legend-node.waiting {
    background: var(--muted);
    opacity: 0.5;
    width: 5px;
    height: 5px;
  }
  .contract-line {
    width: 13px;
    height: 1px;
    background: var(--lavender);
  }
  .network-legend .legend-scale {
    margin-left: auto;
    font-size: 9px;
    color: var(--muted);
  }
  .network-note {
    padding: 0 25px 15px;
    color: var(--muted);
    font-size: 9px;
  }
  .network-tooltip {
    position: absolute;
    pointer-events: none;
    width: 205px;
    border: 1px solid var(--line);
    background: color-mix(in srgb, var(--panel) 96%, transparent);
    box-shadow: 0 8px 30px #0006;
    border-radius: 8px;
    padding: 12px;
    z-index: 4;
    font-size: 11px;
  }
  .network-tooltip strong {
    display: block;
    font-size: 12px;
  }
  .network-tooltip > span {
    color: var(--muted);
    display: block;
    margin: 4px 0 10px;
  }
  .network-tooltip > div {
    display: flex;
    justify-content: space-between;
    color: var(--secondary);
    margin-top: 5px;
  }
  .network-tooltip b {
    font-weight: 500;
    color: var(--text);
  }
  @media (max-width: 650px) {
    .network-header {
      padding: 18px 16px 14px;
    }
    .layer-bar {
      padding-left: 16px;
    }
    .layer-caption {
      display: none;
    }
    .canvas-container {
      min-height: 360px;
      height: 360px;
    }
    .scene-instructions {
      bottom: 62px;
      left: 16px;
    }
    .network-legend {
      padding: 14px 16px 10px;
      gap: 12px;
    }
    .network-legend .legend-scale {
      width: 100%;
      margin-left: 0;
    }
    .network-note {
      padding-left: 16px;
    }
    .scene-caption {
      left: 16px;
    }
  }
</style>
