// The separable-gaussian passes behind CSS `filter: blur()`.
//
// Every pass is one fragment shader over a fullscreen triangle, every target
// is `Rgba8Unorm`, and none of them blends: each writes what it computes.
// The whole chain works in *premultiplied* alpha, which is why the first pass
// exists at all — vello writes its render target unpremultiplied (`fine.wgsl`
// divides by alpha before `textureStore`), and filtering unpremultiplied
// color mixes the color of fully transparent pixels into its neighbours,
// which shows up as a dark halo around a white square on white.
//
// The sampler is supplied per entry and is one of two, both linear. A
// `filter: blur()` group binds the clamp-to-edge one: its bake rect carries a
// >= 3 sigma transparent margin, so what the clamp extends is transparent
// black, which is what filter-effects-1 specifies for a filter region's
// outside. A `backdrop-filter` entry binds the mirror-repeat one: its rect is
// the spec's crop with no margin at all, so a clamp would smear the crop's
// edge row outward and a transparent border would darken it, while mirroring
// reflects the backdrop back in.

struct Params {
    // 1 / source texture size, in texels.
    texel: vec2f,
    // The separable axis as a normalized source-space step of one texel:
    // (texel.x, 0) horizontally, (0, texel.y) vertically.
    axis: vec2f,
    // Destination-to-source pixel-centre scale: 2 for a 2x box downsample,
    // 0.5 for a 2x tent upsample, 1 for a same-size pass.
    ratio: f32,
    // Symmetric taps in use, `tap[0]` being the centre.
    taps: u32,
    _pad: vec2f,
    // (offset in source texels along `axis`, weight) per tap. The weights
    // are normalized so `tap[0].y + 2 * sum(tap[1..].y) == 1`.
    tap: array<vec4f, 4>,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> params: Params;

// One oversized triangle covering the whole clip volume, so no vertex buffer
// and no index buffer exist anywhere in this module.
@vertex
fn vs_full(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    let x = f32(i32(index) / 2) * 4.0 - 1.0;
    let y = f32(i32(index) & 1) * 4.0 - 1.0;
    return vec4f(x, y, 0.0, 1.0);
}

// Source and destination are the same size here, so the texel under this
// fragment is exact and `textureLoad` needs no sampler and no rounding.
@fragment
fn fs_premultiply(@builtin(position) position: vec4f) -> @location(0) vec4f {
    let c = textureLoad(source, vec2i(position.xy), 0);
    return vec4f(c.rgb * c.a, c.a);
}

// One bilinear tap does the whole resample:
//
// - Downsample (ratio 2): the destination pixel centre `p + 0.5` maps to source coordinate
//   `2p + 1`, which is the exact corner shared by the 2x2 source block, so the bilinear weights
//   are 1/4 each and the tap *is* a box average.
// - Upsample (ratio 0.5): the destination centre maps to `(p + 0.5) / 2` in low-resolution texels,
//   which is the standard bilinear tent.
@fragment
fn fs_resample(@builtin(position) position: vec4f) -> @location(0) vec4f {
    let uv = position.xy * params.ratio * params.texel;
    return textureSampleLevel(source, source_sampler, uv, 0.0);
}

// One separable gaussian half-pass. Each non-centre tap is one bilinear
// sample standing in for a pair of adjacent texels, placed at their
// weight-weighted midpoint — so a radius-6 kernel costs 7 taps instead of 13.
@fragment
fn fs_gaussian(@builtin(position) position: vec4f) -> @location(0) vec4f {
    let uv = position.xy * params.texel;
    var sum = params.tap[0].y * textureSampleLevel(source, source_sampler, uv, 0.0);
    for (var i = 1u; i < params.taps; i = i + 1u) {
        let offset = params.axis * params.tap[i].x;
        let weight = params.tap[i].y;
        sum = sum
            + weight * textureSampleLevel(source, source_sampler, uv + offset, 0.0)
            + weight * textureSampleLevel(source, source_sampler, uv - offset, 0.0);
    }
    return sum;
}
