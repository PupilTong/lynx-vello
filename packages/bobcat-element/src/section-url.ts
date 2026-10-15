// The one URL rule a container's sections are named by, shared by both realms.
//
// A container — the page's own, or a lazy one `lynx.fetchBundle` installed —
// answers its bodies and its stylesheets at URLs derived from its own, every
// one of them *under* the container URL's path with its `?#` suffix kept: a
// named section as a `.js` file, a manifest path as the path it is, a named
// stylesheet as `custom_section/<encoded name>.css`, and the page's own
// StyleInfo as `style-info.css`. `bobcat-source` registers exactly these
// strings, for every container through one `ContainerSources`
// (`named_chunk_url`, `bundle_path_url`, `named_style_url` and
// `bundle_style_url` in `crates/bobcat-source/src/container.rs`), so the rule
// lives in one module rather than once per realm — MTS builds them for
// `__LoadLepusChunk`, `lynx.loadScript` and `__LoadStyleSheet`, BTS for
// `lynx.requireModule`, `lynx.loadScript` and `nativeApp.loadScript`.

/**
 * One section name as `ContainerSources` percent-encodes it: `form_urlencoded`'s
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

/**
 * The resource URL one manifest path lives at: the container URL's path, then
 * the bundle path as it is, the `?#` suffix kept.
 *
 * One leading `/` is stripped first, so `/app-service.js` and
 * `app-service.js` name one URL, and no `.js` is appended: a manifest path
 * already names its file, `.json` included, which is what tells the loader to
 * parse it. A `?` or `#` inside the path is escaped, so it stays part of the
 * path rather than starting the container's suffix; everything else is left
 * for the URL parser, which escapes it the way `Url::set_path` does.
 */
export function bundlePathURL(path: string, bundleURL: string): string {
  const [base, suffix] = parts(bundleURL);
  const inside = (path.startsWith("/") ? path.slice(1) : path)
    .replace(/[?#]/g, character => (character === "?" ? "%3F" : "%23"));
  return `${base.replace(/\/$/, "")}/${inside}${suffix}`;
}

/**
 * The resource URL a container's own StyleInfo lives at: the container URL's
 * path, then `style-info.css`, the `?#` suffix kept. A page adopts its own
 * as the view's listed author sheet and a `<frame src>` boot adopts its
 * card's into the frame's shadow root; a lazy component's is registered and
 * never asked for.
 */
export function bundleStyleSheetURL(bundleURL: string): string {
  const [base, suffix] = parts(bundleURL);
  return `${base.replace(/\/$/, "")}/style-info.css${suffix}`;
}
