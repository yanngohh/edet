/** **One tooltip for the whole page.**
 *
 * The browser's own `title` is not a tooltip a reader can rely on: it waits
 * about a second, disappears on its own after a few, cannot be styled, cannot
 * be reached from a keyboard and shows nothing at all on a touch screen. What
 * a refusal code means is worth more than that, so the player draws its own.
 *
 * `use:hint={text}` on any element; a falsy text attaches nothing, so a code
 * the locale has no sentence for stays a plain code rather than an empty
 * bubble. It is positioned in viewport coordinates from the element's own box,
 * which is why no panel's `overflow` can clip it.
 */
import { writable } from "svelte/store";

export const tip = writable(null);

// The bubble belongs to ONE element at a time. Without this, a hint unmounting
// anywhere — and a day step re-renders the whole event list — cleared the
// bubble the reader was pointing at somewhere else, with no way to bring it
// back short of leaving the element and returning to it.
let owner = null;

// One listener for the page, not one per hinted element: a busy day mounts
// dozens, and each was registering its own capture-phase scroll listener.
let listening = false;
const hideAll = () => {
  owner = null;
  tip.set(null);
};

export function hint(node, text) {
  let said = text;

  const place = () => {
    const box = node.getBoundingClientRect();
    const width = 328;
    // Kept inside the window on both axes: a bubble anchored near an edge
    // would otherwise run off the page it is explaining, and below the fold it
    // cannot be scrolled to — scrolling dismisses it.
    const x = Math.max(8, Math.min(box.left, window.innerWidth - width - 8));
    const below = box.bottom + 8;
    const room = window.innerHeight - below;
    return { text: said, x, y: room < 120 ? Math.max(8, box.top - 12 - Math.min(room + 120, 180)) : below };
  };
  const show = () => {
    if (!said) return;
    if (!listening) {
      window.addEventListener("scroll", hideAll, true);
      listening = true;
    }
    owner = node;
    tip.set(place());
  };
  const hide = () => {
    if (owner !== node) return;
    hideAll();
  };

  const apply = (value) => {
    said = value;
    // Only what says something is hoverable, and reachable by tab.
    node.classList.toggle("has-hint", !!said);
    if (said) node.setAttribute("tabindex", "0");
    else node.removeAttribute("tabindex");
    // A bubble already open on this element follows what it says: a day step
    // can change the code under a motionless pointer.
    if (owner === node) {
      if (said) tip.set(place());
      else hideAll();
    }
  };
  apply(text);

  node.addEventListener("pointerenter", show);
  node.addEventListener("pointerleave", hide);
  node.addEventListener("focus", show);
  node.addEventListener("blur", hide);

  return {
    update: apply,
    destroy() {
      hide();
      node.removeEventListener("pointerenter", show);
      node.removeEventListener("pointerleave", hide);
      node.removeEventListener("focus", show);
      node.removeEventListener("blur", hide);
    },
  };
}
