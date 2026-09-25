// Stable 3D coordinates use every member in the tape so scrubbing never
// reshuffles the world. Position is a layout aid, never a measured quantity.
export function networkScales(days) {
  let capacity = 1,
    stake = 1;
  // A single row without `capacity`, or a two-element edge, would otherwise
  // make both scales NaN — and a NaN radius throws inside the canvas gradient,
  // which ends the draw loop for good.
  for (const day of days) {
    for (const member of day.members)
      if (Number.isFinite(member.capacity)) capacity = Math.max(capacity, member.capacity);
    for (const edge of day.edges) if (Number.isFinite(edge[2])) stake = Math.max(stake, edge[2]);
  }
  return { capacity, stake };
}

export function layoutNetwork(days) {
  const members = new Map();
  const links = new Map();
  for (const day of days) {
    for (const member of day.members) members.set(member.id, member);
    for (const [from, to] of day.edges) links.set(`${from}:${to}`, [from, to]);
  }
  const order = (id) => {
    const value = Math.sin((Number(id) + 1) * 12.9898) * 43758.5453;
    return value - Math.floor(value);
  };
  const nodes = [...members.values()]
    .sort((a, b) => order(a.id) - order(b.id))
    .map((member, i, all) => {
      const y = 1 - (2 * (i + 0.5)) / all.length;
      const radius = Math.sqrt(1 - y * y);
      const angle = i * Math.PI * (3 - Math.sqrt(5));
      return {
        id: member.id,
        x: Math.cos(angle) * radius * 180,
        y: y * 170,
        z: Math.sin(angle) * radius * 180,
        // The flat view's own coordinates, filled in by `flatten` below.
        fx: 0,
        fy: 0,
      };
    });
  const byId = new Map(nodes.map((node) => [node.id, node]));
  // Bound the layout cost on large tapes; the spherical distribution itself is
  // already stable and remains readable without force relaxation.
  if (nodes.length <= 220) {
    for (let step = 0; step < 90; step++) {
      const forces = new Map(
        nodes.map((node) => [
          node.id,
          { x: -node.x * 0.018, y: -node.y * 0.018, z: -node.z * 0.018 },
        ]),
      );
      for (let i = 0; i < nodes.length; i++)
        for (let j = i + 1; j < nodes.length; j++) {
          const a = nodes[i],
            b = nodes[j];
          const dx = a.x - b.x,
            dy = a.y - b.y,
            dz = a.z - b.z;
          const distance = Math.hypot(dx, dy, dz) || 1;
          const force = 1500 / (distance * distance);
          for (const [key, delta] of [
            ["x", dx],
            ["y", dy],
            ["z", dz],
          ]) {
            forces.get(a.id)[key] += (delta / distance) * force;
            forces.get(b.id)[key] -= (delta / distance) * force;
          }
        }
      for (const [from, to] of links.values()) {
        const a = byId.get(from),
          b = byId.get(to);
        if (!a || !b || from === to) continue;
        const distance = Math.hypot(a.x - b.x, a.y - b.y, a.z - b.z) || 1;
        const force = (distance - 110) * 0.008;
        for (const key of ["x", "y", "z"]) {
          const delta = ((b[key] - a[key]) / distance) * force;
          forces.get(a.id)[key] += delta;
          forces.get(b.id)[key] -= delta;
        }
      }
      for (const node of nodes)
        for (const key of ["x", "y", "z"]) node[key] += forces.get(node.id)[key] * 0.7;
    }
  }
  const extent = Math.max(1, ...nodes.map((node) => Math.hypot(node.x, node.y, node.z)));
  // An ellipsoidal field uses the wide viewport while keeping true depth.
  for (const node of nodes)
    for (const key of ["x", "y", "z"])
      node[key] = (node[key] / extent) * 205 * (key === "x" ? 1.45 : key === "z" ? 1.15 : 1);
  flatten(nodes, links, byId);
  return byId;
}

/** **A layout for the flat view, laid out flat.**
 *
 * Dropping `z` from a sphere piles the far side onto the near one: two people
 * with nothing to do with each other land on the same pixel, and a reader
 * cannot tell an overlap from a pair. So the 2D view gets coordinates of its
 * own — `fx`, `fy` — relaxed in the plane, where the only thing that has to
 * hold is that nodes do not touch.
 *
 * It is the same kind of claim as the 3D one: position is a layout aid and
 * never a measured quantity. What it must not do is lie by collision.
 */
function flatten(nodes, links, byId) {
  const golden = Math.PI * (3 - Math.sqrt(5));
  nodes.forEach((node, i) => {
    // A phyllotaxis disc: even density, no ring artefacts, stable per index.
    const radius = 205 * Math.sqrt((i + 0.5) / nodes.length);
    node.fx = Math.cos(i * golden) * radius * 1.45;
    node.fy = Math.sin(i * golden) * radius;
  });
  if (nodes.length > 220) return;
  const MIN = 34; // a node is at most ~13px across, plus room for a label
  for (let step = 0; step < 120; step++) {
    const push = new Map(nodes.map((n) => [n.id, { x: -n.fx * 0.004, y: -n.fy * 0.004 }]));
    for (let i = 0; i < nodes.length; i++)
      for (let j = i + 1; j < nodes.length; j++) {
        const a = nodes[i], b = nodes[j];
        const dx = a.fx - b.fx, dy = a.fy - b.fy;
        const distance = Math.hypot(dx, dy) || 0.01;
        // Ordinary repulsion, plus a hard shove for anything inside MIN: the
        // one thing this layout owes the reader is that two people are two
        // dots.
        const force = 900 / (distance * distance) + (distance < MIN ? (MIN - distance) * 0.9 : 0);
        push.get(a.id).x += (dx / distance) * force;
        push.get(a.id).y += (dy / distance) * force;
        push.get(b.id).x -= (dx / distance) * force;
        push.get(b.id).y -= (dy / distance) * force;
      }
    for (const [from, to] of links.values()) {
      const a = byId.get(from), b = byId.get(to);
      if (!a || !b || from === to) continue;
      const distance = Math.hypot(a.fx - b.fx, a.fy - b.fy) || 1;
      const force = (distance - 120) * 0.006;
      push.get(a.id).x += ((b.fx - a.fx) / distance) * force;
      push.get(a.id).y += ((b.fy - a.fy) / distance) * force;
      push.get(b.id).x -= ((b.fx - a.fx) / distance) * force;
      push.get(b.id).y -= ((b.fy - a.fy) / distance) * force;
    }
    for (const node of nodes) {
      node.fx += push.get(node.id).x * 0.5;
      node.fy += push.get(node.id).y * 0.5;
    }
  }
  const wide = Math.max(1, ...nodes.map((n) => Math.abs(n.fx))) / 1.45;
  const tall = Math.max(1, ...nodes.map((n) => Math.abs(n.fy)));
  const scale = 205 / Math.max(wide, tall);
  for (const node of nodes) {
    node.fx *= scale;
    node.fy *= scale;
  }
}

/**
 * **Where the people without an account are drawn.**
 *
 * A person the ledger has no row for has no position in the layout, which is
 * built from members and stakes. They are drawn as small satellites: around
 * their sponsor where the sponsor is a member on this day — the offer that
 * introduced them is the one relation the tape records — and on an outer ring
 * around the whole field where nobody introduced them. Position is a layout
 * aid, as everywhere here; the ring says only "not seated".
 */
export function satellitesOf(run, day, positions) {
  const out = [];
  const seated = new Map(day.members.filter((m) => m.person != null).map((m) => [m.person, m.id]));
  const persons = (run?.persons || []).filter(
    (p) => (p.joined_tick ?? 0) <= (day.tick ?? 0) && !seated.has(p.index),
  );
  const around = new Map();
  const loose = [];
  for (const p of persons) {
    const anchor = p.introduced_by == null ? null : seated.get(p.introduced_by);
    const at = anchor == null ? null : positions.get(anchor);
    if (at) {
      if (!around.has(anchor)) around.set(anchor, []);
      around.get(anchor).push(p);
    } else loose.push(p);
  }
  const RING = 26;
  for (const [anchor, group] of around) {
    const at = positions.get(anchor);
    group.forEach((p, k) => {
      const angle = (k / group.length) * Math.PI * 2 + anchor * 0.7;
      out.push({
        person: p.index,
        sponsor: anchor,
        x: at.x + Math.cos(angle) * RING,
        y: at.y + Math.sin(angle * 1.3) * RING * 0.6,
        z: at.z + Math.sin(angle) * RING,
        fx: (at.fx ?? at.x) + Math.cos(angle) * RING * 1.2,
        fy: (at.fy ?? at.y) + Math.sin(angle) * RING * 0.9,
      });
    });
  }
  loose.forEach((p, k) => {
    const angle = (k / Math.max(loose.length, 1)) * Math.PI * 2;
    out.push({
      person: p.index,
      sponsor: null,
      x: Math.cos(angle) * 250 * 1.45,
      y: Math.sin(angle * 2) * 30,
      z: Math.sin(angle) * 250 * 1.15,
      fx: Math.cos(angle) * 250 * 1.45,
      fy: Math.sin(angle) * 235,
    });
  });
  return out;
}

/// **When a scene that turns by itself should stop turning.**
///
/// The orbit is the one thing that redraws without being asked, so on a big
/// run or a slow machine it is what makes the page stop answering the scrub
/// bar. What decides is the cost of a DRAW and never a count of edges: heavy
/// is the machine and the scene together, and only the clock knows.
///
/// A smoothed cost, so one slow frame — a garbage collection, a tab coming
/// back — decides nothing, and a run of them, so a scene that is merely
/// occasionally slow keeps turning. Stopping is one way: a reader who wants it
/// back has the button, and a scene that stopped and started on its own would
/// be worse than either.
export const ORBIT_LAG_MS = 28;
export const ORBIT_LAG_FRAMES = 45;

export const orbitWatch = (state, costMs, lagMs = ORBIT_LAG_MS, frames = ORBIT_LAG_FRAMES) => {
  const ema = state.ema ? state.ema * 0.85 + costMs * 0.15 : costMs;
  const strikes = ema > lagMs ? state.strikes + 1 : 0;
  return { ema, strikes: strikes >= frames ? 0 : strikes, stop: strikes >= frames };
};

export const orbitRested = () => ({ ema: 0, strikes: 0, stop: false });
