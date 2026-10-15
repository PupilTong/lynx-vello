// The one URL rule a container's sections are named by, as a table.
//
// `bobcat-source` registers every body and stylesheet of a container at
// exactly these strings and the realms ask for them by these, so the two
// halves agree only while the rule does: the same rows are pinned on the Rust
// side, in `crates/bobcat-source/src/container.rs`'s unit tests. Each row
// keeps the container's `?#` suffix after what it adds, drops a trailing `/`
// of the container's path, and encodes a section name the way
// `form_urlencoded` does, with a space as `%20`.

import { describe, expect, it } from "@rstest/core";
import {
  bundlePathURL,
  bundleStyleSheetURL,
  sectionURL,
  styleSheetURL,
} from "../src/section-url.ts";

const BUNDLE = "https://cdn.test/app/card.web.bundle";

describe("the container URL rule", () => {
  it("names a section as an encoded .js file under the container", () => {
    expect(sectionURL("background", BUNDLE)).toBe(`${BUNDLE}/background.js`);
    expect(sectionURL("/background", BUNDLE)).toBe(`${BUNDLE}/background.js`);
    expect(sectionURL("A &/中", BUNDLE)).toBe(`${BUNDLE}/A%20%26%2F%E4%B8%AD.js`);
    expect(sectionURL("it's (x)!~*", BUNDLE)).toBe(`${BUNDLE}/it%27s%20%28x%29%21%7E*.js`);
    expect(sectionURL("main-thread", `${BUNDLE}?v=1#f`)).toBe(`${BUNDLE}/main-thread.js?v=1#f`);
    expect(sectionURL("root", `${BUNDLE}/`)).toBe(`${BUNDLE}/root.js`);
  });

  it("names a manifest path as the path it is under the container", () => {
    expect(bundlePathURL("/app-service.js", BUNDLE)).toBe(`${BUNDLE}/app-service.js`);
    expect(bundlePathURL("app-service.js", BUNDLE)).toBe(`${BUNDLE}/app-service.js`);
    // No `.js` is appended: a manifest path already names its file.
    expect(bundlePathURL("/data.json", BUNDLE)).toBe(`${BUNDLE}/data.json`);
    expect(bundlePathURL("/chunk.js", BUNDLE)).toBe(`${BUNDLE}/chunk.js`);
    expect(bundlePathURL("/a/b/c.js", `${BUNDLE}?rev=1#f`)).toBe(`${BUNDLE}/a/b/c.js?rev=1#f`);
    expect(bundlePathURL("/x.js", `${BUNDLE}/`)).toBe(`${BUNDLE}/x.js`);
    // One leading `/` only.
    expect(bundlePathURL("//x.js", BUNDLE)).toBe(`${BUNDLE}//x.js`);
    // A `?` or `#` inside the path stays in the path.
    expect(bundlePathURL("/q?x#y.js", `${BUNDLE}?rev=1`)).toBe(`${BUNDLE}/q%3Fx%23y.js?rev=1`);
    // A rooted container URL stays rooted, for the host to resolve.
    expect(bundlePathURL("/background", "/lazy/child.bundle")).toBe("/lazy/child.bundle/background");
  });

  it("names a named stylesheet under custom_section/", () => {
    expect(styleSheetURL("CSS", BUNDLE)).toBe(`${BUNDLE}/custom_section/CSS.css`);
    expect(styleSheetURL("A &/中", `${BUNDLE}/#entry`))
      .toBe(`${BUNDLE}/custom_section/A%20%26%2F%E4%B8%AD.css#entry`);
  });

  it("names the page's own StyleInfo style-info.css", () => {
    expect(bundleStyleSheetURL(BUNDLE)).toBe(`${BUNDLE}/style-info.css`);
    expect(bundleStyleSheetURL(`${BUNDLE}?rev=1`)).toBe(`${BUNDLE}/style-info.css?rev=1`);
    expect(bundleStyleSheetURL(`${BUNDLE}/#f`)).toBe(`${BUNDLE}/style-info.css#f`);
    // Written as a `<frame src>` names it: a relative container stays
    // relative, for the realm's base to resolve.
    expect(bundleStyleSheetURL("./card.web.bundle?rev=2"))
      .toBe("./card.web.bundle/style-info.css?rev=2");
    expect(sectionURL("root", "./card.web.bundle?rev=2")).toBe("./card.web.bundle/root.js?rev=2");
  });
});
