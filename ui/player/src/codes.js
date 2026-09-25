/** **What the ledger's refusal codes mean.**
 *
 * `ET-BND-001` is a fact about a transaction; it is not English. The English
 * is the wallet's own, injected at build time from `ui/wallet/src/locales/en.json`
 * (see `vite.config.js`) — never copied into this app, so it cannot drift from
 * the sentence a member reads when the same refusal reaches them.
 *
 * A code with no entry gets nothing rather than a guess: the locale is the
 * only source, and a missing sentence means the ledger grew a code the client
 * has not been taught yet.
 */

// `define` substitutes the map as a literal at build time. Outside a build —
// `node --test` over these modules — there is no substitution and no locale,
// so a code simply has no sentence rather than the app failing to load.
const english = typeof __EDET_CODES__ === "undefined" ? {} : __EDET_CODES__;

/** The wallet's sentence for a code, or null where the locale has none. */
export function meaning(code) {
  return (code && english[code]) || null;
}

/** The first clause of that sentence — a line for a place a paragraph will not
 *  fit. Never a rewrite: it stops at the sentence's own punctuation. */
export function gist(code) {
  const said = meaning(code);
  if (!said) return null;
  const stop = said.search(/[.;—]/);
  return stop > 20 ? said.slice(0, stop + (said[stop] === "." ? 1 : 0)).trim() : said;
}

/** A sentence split around the refusal codes in it, so only the code carries
 *  the hover and the words around it stay ordinary text. A code the locale has
 *  no sentence for comes back as plain text: nothing to hover, nothing to say. */
export function segments(text) {
  const out = [];
  let at = 0;
  for (const m of String(text ?? "").matchAll(/ET-[A-Z]+-\d+/g)) {
    if (!meaning(m[0])) continue;
    if (m.index > at) out.push({ text: text.slice(at, m.index) });
    out.push({ text: m[0], code: m[0] });
    at = m.index + m[0].length;
  }
  if (at < String(text ?? "").length) out.push({ text: text.slice(at) });
  return out;
}
