// The one URL rule a container's sections are named by, shared by both realms.
//
// A container — the page's own, or a lazy one `lynx.fetchBundle` installed —
// answers its bodies and its stylesheets at URLs derived from its own: the
// section name as a `.js` file under the container URL's path, and the named
// stylesheet as `custom_section/<encoded name>.css` under it. `bobcat-source`
// registers exactly these strings (`named_chunk_url` and `named_style_url` in
// `crates/bobcat-source/src/page.rs`), so the rule lives in one module rather
// than once per realm — MTS builds them for `__LoadLepusChunk`,
// `lynx.loadScript` and `__LoadStyleSheet`, BTS for a lazy `bundleName`.

/**
 * One section name as `PageSource` percent-encodes it: `form_urlencoded`'s
 * byte serializer, which escapes `!~'()` where `encodeURIComponent` leaves
 * them, and writes a space as `%20` rather than `+`.
 */
export function encodedSection(key: string): string {
  return encodeURIComponent(key).replace(/[!~'()]/g,
    character => `%${character.charCodeAt(0).toString(16).toUpperCase()}`);
}

/** A container URL's path and its `?#` suffix, which a section URL goes between. */
function parts(bundleURL: string): [string, string] {
  const suffixAt = bundleURL.search(/[?#]/);
  return suffixAt < 0
    ? [bundleURL, ""]
    : [bundleURL.slice(0, suffixAt), bundleURL.slice(suffixAt)];
}

/**
 * The resource URL one named section lives at: the container URL's path, then
 * the encoded section name as a `.js` file, the `?#` suffix kept.
 *
 * One leading `/` is stripped first, so `background` and `/background` — the
 * two spellings a container carries one body under — name one URL.
 */
export function sectionURL(name: string, bundleURL: string): string {
  const [path, suffix] = parts(bundleURL);
  const section = name.startsWith("/") ? name.slice(1) : name;
  return `${path.replace(/\/$/, "")}/${encodedSection(section)}.js${suffix}`;
}

/**
 * The resource URL one named stylesheet lives at: the container URL's path,
 * then `custom_section/` and the encoded section name as a `.css` file, the
 * `?#` suffix kept — the compiler's `CSS` section is `custom_section/CSS.css`.
 */
export function styleSheetURL(key: string, bundleURL: string): string {
  const [path, suffix] = parts(bundleURL);
  return `${path.replace(/\/$/, "")}/custom_section/${encodedSection(key)}.css${suffix}`;
}
