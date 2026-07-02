# Test fixtures

Real web binary bundles built by the lynx-stack e2e suite
(`packages/web-platform/web-core-e2e/dist/` in
<https://github.com/lynx-family/lynx-stack>, Apache-2.0). Vendored build
artifacts, unmodified.

| File | Why it's here |
| --- | --- |
| `config-lazy-component-css.web.bundle` | Small lazy component (`isLazy: true`) with a real CSS style sheet. |
| `basic-bindtap.web.bundle` | Regular card (`isLazy: false`) with an effectively empty StyleInfo map. |
| `basic-performance-large-css.web.bundle` | 24 KB StyleInfo section — stress test for the rkyv decode path. |
