//! The `vello_gpu` rasterizer: a composed [`vello::Scene`] replayed into a
//! `vello_gpu::Scene` and rendered by sparse strips on `vello_gpu`'s wgpu
//! backend. Compiled with the `vello_gpu` feature; [`super::raster::Raster`]
//! is its only caller.
//!
//! The paint and compose layers build a `vello::Scene` either way. This
//! module decodes its encoding stream by stream — paths, styles,
//! transforms, draw objects and the late-bound patches (ramps, images, glyph
//! runs) — and replays it into a `vello_gpu::Scene` of the target's size.
//! An image whose handle has a texture bound ([`SparseBackend::bind`]: the
//! `filter: blur()`, `backdrop-filter` and vector-image bakes) is drawn as an
//! external texture, sampled in place; any other bitmap is uploaded to
//! `vello_gpu`'s image atlas once per blob.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    reason = "a decoder over vello's packed streams: counts and offsets are small exact integers"
)]

use std::time::{Duration, Instant};

mod flatten;
mod glyphs;

use rustc_hash::FxHashMap;
use vello::kurbo::{Affine, BezPath, Point, Rect, Shape as _};
use vello::low_level::vello_encoding::{
    DrawBeginClip, DrawTag, Encoding, Patch, PathTag, Style, Transform,
};
use vello::peniko::color::{HueDirection, PremulRgba8, Srgb};
use vello::peniko::{
    self, BlendMode, Color, Compose, Extend, Fill, Gradient, ImageAlphaType, ImageData,
    ImageFormat, ImageQuality, ImageSampler, Mix,
};
use vello::wgpu;
use vello_gpu::{
    ClearSettings, Image, ImageId, ImageSource, PixelMetadata, Pixmap, RectU16, RenderSize,
    RenderTargetConfig, Renderer, Resources, Scene, TargetInit, TextureBindings, TextureId,
};

/// What one frame cost, and what it could not draw.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Stats {
    /// Decoding the vello encoding and building the sparse-strip scene
    /// (path flattening and strip generation run here, on the CPU).
    pub(crate) encode: Duration,
    /// Of `encode`, decoding vello's streams alone (the trial's own cost).
    pub(crate) decode: Duration,
    /// `Renderer::render` and the submit.
    pub(crate) render: Duration,
    pub(crate) draws: u32,
    pub(crate) glyph_runs: u32,
    pub(crate) layers: u32,
    /// Image draws with neither a bound texture nor readable pixels.
    pub(crate) images_skipped: u32,
    pub(crate) masks_skipped: u32,
}

pub(crate) struct SparseBackend {
    renderer: Renderer,
    resources: Resources,
    scene: Scene,
    /// Uploaded bitmaps, by blob id and size.
    images: FxHashMap<(u64, u32, u32), ImageId>,
    /// Textures bound to image handles, by blob id.
    bound: FxHashMap<u64, Bound>,
    next_texture: u64,
    staircases: Staircases,
    /// Units per em of each font a glyph run named, by blob id and index.
    units_per_em: FxHashMap<(u64, u32), f64>,
    /// glifo's outline and hinting caches for the runs drawn through
    /// [`glyphs::Glyphs`].
    glyph_prep: glifo::GlyphPrepCache,
    pub(crate) last: Stats,
}

/// A texture bound to an image handle.
struct Bound {
    id: TextureId,
    view: wgpu::TextureView,
    width: u16,
    height: u16,
}

impl std::fmt::Debug for SparseBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SparseBackend")
            .field("last", &self.last)
            .finish()
    }
}

/// One decoded path, flattened to device space as vello flattens it.
struct DecodedPath {
    /// The lines vello rasterizes for the path, filled under [`Self::fill`].
    path: BezPath,
    /// The transform current at the path's end marker: the brush's.
    brush_transform: Affine,
    style: Style,
}

impl DecodedPath {
    /// The rule `path` fills under: a stroke's lines are filled non-zero.
    fn fill(&self) -> Fill {
        if is_stroke(&self.style) {
            Fill::NonZero
        } else {
            fill_rule(&self.style)
        }
    }
}

fn affine(transform: &Transform) -> Affine {
    let [a, b, c, d] = transform.matrix;
    let [e, f] = transform.translation;
    Affine::new([
        f64::from(a),
        f64::from(b),
        f64::from(c),
        f64::from(d),
        f64::from(e),
        f64::from(f),
    ])
}

/// `run`'s synthetic bold as glifo has to be asked for it to widen each
/// outline as vello does, `full` being the run's transform with its glyph
/// transform.
///
/// vello widens the outline at the run's font size, before any transform.
/// glifo widens it at the size it caches the outline at: units per em for an
/// unhinted fill, the font size the run's uniform scale folds into
/// otherwise (`glifo::glyph::GlyphScaleProperties`). The amount and its
/// tolerance are rescaled by the ratio of the two sizes.
fn embolden(
    run: &vello::low_level::vello_encoding::GlyphRun,
    full: Affine,
    units_per_em: &mut FxHashMap<(u64, u32), f64>,
) -> glifo::FontEmbolden {
    let mut embolden = glifo::FontEmbolden {
        amount: run.font_embolden.amount,
        join: run.font_embolden.join,
        miter_limit: run.font_embolden.miter_limit,
        tolerance: run.font_embolden.tolerance,
    };
    if embolden.amount.xx == 0.0 && embolden.amount.yy == 0.0 {
        return embolden;
    }
    let [a, b, c, d, ..] = full.as_coeffs();
    let uniform = (a - d).abs() <= 1.0 / 4096.0 && a > 0.0 && b == 0.0 && c == 0.0;
    let fill = matches!(run.style, peniko::Style::Fill(_));
    let scale = if fill && !run.hint {
        let upem = *units_per_em
            .entry((run.font.data.id(), run.font.index))
            .or_insert_with(|| head_units_per_em(run.font.data.data(), run.font.index));
        upem / f64::from(run.font_size)
    } else if uniform {
        a
    } else {
        1.0
    };
    embolden.amount =
        vello::kurbo::Diagonal2::new(embolden.amount.xx * scale, embolden.amount.yy * scale);
    embolden.tolerance *= scale;
    embolden
}

/// The `unitsPerEm` of face `index` in `font`'s `head` table, or 1000 for
/// a font that has none.
fn head_units_per_em(font: &[u8], index: u32) -> f64 {
    let u16_at = |at: usize| {
        font.get(at..at + 2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
    };
    let u32_at = |at: usize| {
        font.get(at..at + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    let face = if font.get(..4) == Some(b"ttcf") {
        u32_at(12 + 4 * index as usize).map(|offset| offset as usize)
    } else {
        Some(0)
    };
    face.and_then(|face| {
        let tables = usize::from(u16_at(face + 4)?);
        (0..tables).find_map(|table| {
            let record = face + 12 + 16 * table;
            (font.get(record..record + 4)? == b"head")
                .then(|| u16_at(u32_at(record + 8)? as usize + 18))
                .flatten()
        })
    })
    .map_or(1000.0, f64::from)
}

/// `transform` back in the `f32` it was encoded in.
fn f32_transform(transform: Affine) -> Transform {
    let [a, b, c, d, e, f] = transform.as_coeffs().map(|v| v as f32);
    Transform {
        matrix: [a, b, c, d],
        translation: [e, f],
    }
}

fn is_stroke(style: &Style) -> bool {
    style.flags_and_miter_limit & Style::FLAGS_STYLE_BIT != 0
}

fn fill_rule(style: &Style) -> Fill {
    if style.flags_and_miter_limit & Style::FLAGS_FILL_BIT == 0 {
        Fill::NonZero
    } else {
        Fill::EvenOdd
    }
}

/// Every path of `encoding`, in path order, as `PathMonoid` counts them.
fn decode_paths(encoding: &Encoding) -> Vec<DecodedPath> {
    let mut out = Vec::with_capacity(encoding.n_paths as usize);
    let mut transform = Transform::IDENTITY;
    let mut style = Style::from_fill(Fill::NonZero);
    let mut transforms = encoding.transforms.iter();
    let mut styles = encoding.styles.iter();
    let mut word = 0_usize;
    let mut segments: Vec<flatten::Segment> = Vec::new();
    let mut path_transform: Option<Transform> = None;
    let read = |word: usize, is_f32: bool| -> [f32; 2] {
        if is_f32 {
            [
                f32::from_bits(encoding.path_data[word]),
                f32::from_bits(encoding.path_data[word + 1]),
            ]
        } else {
            let w = encoding.path_data[word];
            [f32::from((w & 0xffff) as i16), f32::from((w >> 16) as i16)]
        }
    };
    for tag in &encoding.path_tags {
        let t = tag.0;
        if t & PathTag::TRANSFORM.0 != 0 {
            if let Some(next) = transforms.next() {
                transform = *next;
            }
        }
        if t & PathTag::STYLE.0 != 0 {
            if let Some(next) = styles.next() {
                style = *next;
            }
        }
        let segment = t & 0x3;
        if segment != 0 {
            let is_f32 = t & 0x8 != 0;
            let size = if is_f32 { 2 } else { 1 };
            if path_transform.is_none() {
                path_transform = Some(transform);
            }
            let mut points = [[0.0; 2]; 4];
            for (k, point) in points.iter_mut().enumerate().take(segment as usize + 1) {
                *point = read(word + k * size, is_f32);
            }
            segments.push(flatten::Segment { tag: t, points });
            let end = (t >> 2) & 1;
            word += (segment as usize + end as usize) * size;
        }
        if t & PathTag::PATH.0 != 0 {
            let local = path_transform.take().unwrap_or(transform);
            out.push(DecodedPath {
                path: flatten::flatten(&segments, &style, &local),
                brush_transform: affine(&transform),
                style,
            });
            segments.clear();
        }
    }
    out
}

/// The paint transform, relative to the path transform `transform`, that
/// samples a paint whose device transform is `brush` the way vello's fine
/// stage does for gradients and blurred rectangles: at each pixel's top-left
/// corner. `vello_gpu` samples every paint at the pixel centre, so the paint
/// is moved half a pixel right and down to land the same value there.
fn corner_sampled(transform: Affine, brush: Affine) -> Affine {
    transform.inverse() * Affine::translate((0.5, 0.5)) * brush
}

/// Samples in vello's gradient ramp (`vello_encoding`'s `N_SAMPLES`); its
/// fine stage reads sample `round(t × 511)`.
const RAMP_SAMPLES: usize = 512;
/// The lookup-table width `vello_gpu` builds for a gradient with two stops at
/// one offset (`vello_common::encode::MAX_GRADIENT_LUT_SIZE`); its shader
/// reads entry `floor(t × 4095)`.
const LUT_WIDTH: usize = 4096;
/// How far, in table entries, a parameter on a boundary between two ramp
/// samples is pushed towards the upper one. vello's own parameter there
/// lands on the boundary within rounding and reads the upper sample; the push
/// keeps `vello_gpu`'s, computed another way, from landing below it.
const TIE_BIAS: f64 = 1.0 / 16.0;
/// The constant `vello_gpu`'s shader adds to a linear gradient's parameter.
const LINEAR_SHADER_BIAS: f64 = 1e-5;

/// A gradient's geometry as vello's draw data holds it.
#[derive(Clone, Copy, Debug)]
enum RampGeometry {
    Linear(Point, Point),
    Radial {
        c0: Point,
        r0: f64,
        c1: Point,
        r1: f64,
    },
    /// Angles in turns.
    Sweep {
        center: Point,
        t0: f64,
        t1: f64,
    },
}

/// The affine map from vello's gradient parameter `t` to the one `vello_gpu`
/// is handed, `scale × t + bias`, under the extend both apply to it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct RampMap {
    scale: f64,
    bias: f64,
    extend: Extend,
}

impl RampMap {
    /// The map under which `vello_gpu`'s table entries line up with vello's
    /// ramp samples.
    ///
    /// - Pad: `floor(4095 × (4088 t + β) / 4095) = floor(8 (511 t + ½) − 4 + β)`, so eight entries
    ///   cover each sample and change exactly where `round(511 t)` does.
    /// - Repeat: the period has to stay 1, and 4095 is not a multiple of 511. Shifting by half a
    ///   sample moves `vello_gpu`'s wrap onto vello's (`round` reads sample 511 over the last half
    ///   sample of a period), and an entry a sample boundary crosses reads the upper sample.
    /// - Reflect: no shift commutes with the mirror; entries read the sample at their centre.
    fn new(extend: Extend) -> Self {
        let tie = TIE_BIAS / (LUT_WIDTH - 1) as f64;
        match extend {
            Extend::Pad => Self {
                scale: (8 * (RAMP_SAMPLES - 1)) as f64 / (LUT_WIDTH - 1) as f64,
                bias: tie,
                extend,
            },
            Extend::Repeat => Self {
                scale: 1.0,
                bias: 0.5 / (RAMP_SAMPLES - 1) as f64 + tie,
                extend,
            },
            Extend::Reflect => Self {
                scale: 1.0,
                bias: 0.0,
                extend,
            },
        }
    }

    /// vello's parameter where `vello_gpu`'s is `u`.
    fn source(self, u: f64) -> f64 {
        (u - self.bias) / self.scale
    }

    /// The ramp samples vello reads at the lower and the upper edge of table
    /// entry `k`: equal unless a sample boundary crosses the entry.
    fn samples(self, k: usize) -> (usize, usize) {
        let at = |u: f64| {
            let x = match self.extend {
                Extend::Pad => self.source(u).clamp(0.0, 1.0),
                Extend::Repeat => {
                    let t = self.source(u);
                    t - t.floor()
                }
                Extend::Reflect => u,
            };
            ((x * (RAMP_SAMPLES - 1) as f64 + 0.5).floor() as usize).min(RAMP_SAMPLES - 1)
        };
        let last = (LUT_WIDTH - 1) as f64;
        if self.extend == Extend::Reflect {
            let centre = at((k as f64 + 0.5) / last);
            return (centre, centre);
        }
        (
            at((k as f64 + 1e-6) / last),
            at((k as f64 + 1.0 - 1e-6) / last),
        )
    }
}

/// A linear gradient's parameter as vello's GPU stages compute it: the line
/// `draw_leaf` derives from the device-space end points, evaluated by `fine`
/// at a pixel's top-left corner, in `f32` and in the shaders' order, each
/// product folded into the sum after it as the shader compiler fuses them
/// (unfused, a tie at `x = 91` in a 54 px mask at 64 rounds the other way
/// from vello's).
#[derive(Clone, Copy, Debug)]
struct VelloLine {
    x: f32,
    y: f32,
    c: f32,
}

impl VelloLine {
    fn new(transform: &Transform, p0: [f32; 2], p1: [f32; 2]) -> Self {
        let [a, b, c, d] = transform.matrix;
        let [e, f] = transform.translation;
        let apply =
            |[x, y]: [f32; 2]| [a.mul_add(x, c.mul_add(y, e)), b.mul_add(x, d.mul_add(y, f))];
        let (p0, p1) = (apply(p0), apply(p1));
        let (dx, dy) = (p1[0] - p0[0], p1[1] - p0[1]);
        let scale = 1.0 / dx.mul_add(dx, dy * dy);
        let (x, y) = (dx * scale, dy * scale);
        Self {
            x,
            y,
            c: -p0[0].mul_add(x, p0[1] * y),
        }
    }

    /// The parameter at corner (`x`, `y`): `fine` evaluates it at the first
    /// of the four pixels a thread owns and steps along x from there.
    fn at(self, x: i32, y: i32) -> f32 {
        let base = x.div_euclid(4) * 4;
        let d = self
            .x
            .mul_add(base as f32, self.y.mul_add(y as f32, self.c));
        self.x.mul_add((x - base) as f32, d)
    }

    /// The same parameter in exact arithmetic over the line's coefficients.
    fn exact(self, x: f64, y: f64) -> f64 {
        f64::from(self.x) * x + f64::from(self.y) * y + f64::from(self.c)
    }
}

/// The ramp sample vello's `fine` reads at parameter `t`.
fn vello_sample(t: f32, extend: Extend) -> usize {
    let x = match extend {
        Extend::Pad => t.clamp(0.0, 1.0),
        Extend::Repeat => t - t.floor(),
        Extend::Reflect => (t - 2.0 * (0.5 * t).round_ties_even()).abs(),
    };
    ((x * (RAMP_SAMPLES - 1) as f32).round_ties_even().max(0.0) as usize).min(RAMP_SAMPLES - 1)
}

/// The integer pixel-corner bounds a draw covers, inclusive.
#[derive(Clone, Copy, Debug)]
struct Corners {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

/// A linear gradient draw's device-space line and the corners it covers,
/// which decide how its parameter breaks ties.
#[derive(Clone, Copy, Debug)]
struct LinearDraw {
    line: VelloLine,
    corners: Corners,
}

/// Ramp samples whose boundary vello's parameter, at the corners a draw
/// covers, falls below more often than above (bit `m`: the boundary below
/// sample `m`).
type Lowered = [u64; RAMP_SAMPLES / 64];

/// Corners examined per gradient draw before the tie search stops.
const TIE_BUDGET: u32 = 1 << 16;

/// How vello breaks the ties at a linear gradient draw's corners.
#[derive(Debug, Default)]
struct Ties {
    /// The side most of each boundary's ties fall to.
    lowered: Lowered,
    /// Boundaries some of whose ties fall to the other side.
    split: Lowered,
    /// The pixels those ties belong to, in device space.
    minority: Vec<Rect>,
}

/// How vello breaks ties at `draw`'s corners.
///
/// A corner exactly on a boundary of a hard step (`50%`, at a pixel edge)
/// gets vello's sample by the last bit of its `f32` parameter. The table
/// `vello_gpu` reads sends every tie at one boundary to one side, so the
/// boundary's corners are found on the line where the exact parameter is
/// the tie, vello's own parameter is computed for each, and the table takes
/// the majority; the pixels of the others are drawn again from a second
/// table (see [`Split`]).
fn ties(draw: &LinearDraw, ramp: &[PremulRgba8], extend: Extend) -> Ties {
    let mut out = Ties::default();
    let LinearDraw { line, corners } = *draw;
    let finite = line.x.is_finite() && line.y.is_finite() && line.c.is_finite();
    if extend == Extend::Reflect || !finite || (line.x == 0.0 && line.y == 0.0) {
        return out;
    }
    let ts = [
        line.exact(f64::from(corners.x0), f64::from(corners.y0)),
        line.exact(f64::from(corners.x1), f64::from(corners.y0)),
        line.exact(f64::from(corners.x0), f64::from(corners.y1)),
        line.exact(f64::from(corners.x1), f64::from(corners.y1)),
    ];
    let t_min = ts.iter().copied().fold(f64::INFINITY, f64::min);
    let t_max = ts.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let half_entry = 0.5 / (LUT_WIDTH - 1) as f64;
    // Walk the axis the line varies along least; a line that does not vary
    // along it at all has one answer for the whole axis.
    let along_x = line.x.abs() >= line.y.abs();
    let (cross, steady) = if along_x {
        (line.y, line.x)
    } else {
        (line.x, line.y)
    };
    let (from, to) = if along_x {
        (corners.y0, corners.y1)
    } else {
        (corners.x0, corners.x1)
    };
    let rows = if cross == 0.0 { from..=from } else { from..=to };
    let mut budget = TIE_BUDGET;
    let mut found: Vec<(i32, i32, bool)> = Vec::new();
    for m in 0..RAMP_SAMPLES {
        let below = if m == 0 {
            if extend != Extend::Repeat {
                continue;
            }
            RAMP_SAMPLES - 1
        } else {
            m - 1
        };
        let (lo, hi) = (ramp[below], ramp[m]);
        let step = [
            lo.r.abs_diff(hi.r),
            lo.g.abs_diff(hi.g),
            lo.b.abs_diff(hi.b),
            lo.a.abs_diff(hi.a),
        ];
        if step.into_iter().max().unwrap_or(0) <= 2 {
            continue;
        }
        let within = if m == 0 {
            0.0
        } else {
            (m as f64 - 0.5) / (RAMP_SAMPLES - 1) as f64
        };
        let periods = match extend {
            Extend::Repeat => (t_min - within).ceil() as i64..=(t_max - within).floor() as i64,
            _ => 0..=0,
        };
        found.clear();
        'ties: for period in periods {
            let tie = period as f64 + within;
            if !(t_min..=t_max).contains(&tie) {
                continue;
            }
            for row in rows.clone() {
                if budget == 0 {
                    break 'ties;
                }
                budget -= 1;
                let at = (tie - f64::from(line.c) - f64::from(cross) * f64::from(row))
                    / f64::from(steady);
                for column in [at.floor(), at.floor() + 1.0] {
                    let (x, y) = if along_x {
                        (column, f64::from(row))
                    } else {
                        (f64::from(row), column)
                    };
                    let inside = (f64::from(corners.x0)..=f64::from(corners.x1)).contains(&x)
                        && (f64::from(corners.y0)..=f64::from(corners.y1)).contains(&y);
                    if !inside || (line.exact(x, y) - tie).abs() > half_entry {
                        continue;
                    }
                    let sample = vello_sample(line.at(x as i32, y as i32), extend);
                    if sample == m || sample == below {
                        found.push((x as i32, y as i32, sample == below));
                    }
                }
            }
        }
        let down = found.iter().filter(|(.., down)| *down).count();
        let up = found.len() - down;
        let lowered = down > up;
        if lowered {
            out.lowered[m / 64] |= 1 << (m % 64);
        }
        if up.min(down) > 0 {
            out.split[m / 64] |= 1 << (m % 64);
            if cross == 0.0 {
                // Every row of a column (or column of a row) reads the same.
                for (x, y, down) in &found {
                    if *down != lowered {
                        let (x, y) = (f64::from(*x), f64::from(*y));
                        out.minority.push(if along_x {
                            Rect::new(x, f64::from(corners.y0), x + 1.0, f64::from(corners.y1))
                        } else {
                            Rect::new(f64::from(corners.x0), y, f64::from(corners.x1), y + 1.0)
                        });
                    }
                }
            } else {
                out.minority
                    .extend(found.iter().filter(|(.., down)| *down != lowered).map(
                        |&(x, y, _)| {
                            let (x, y) = (f64::from(x), f64::from(y));
                            Rect::new(x, y, x + 1.0, y + 1.0)
                        },
                    ));
            }
        }
    }
    out
}

/// Gradients replayed so `vello_gpu` samples vello's ramp.
///
/// vello resolves a gradient's stops to a 512-sample ramp and reads sample
/// `round(t × 511)`; `vello_gpu` interpolates its own table of up to 4096
/// entries and reads entry `floor(t × 4095)`. At a hard stop the two
/// disagree by a whole pixel. So the replay computes vello's ramp itself
/// (the same `make_ramp`), hands `vello_gpu` a staircase of stops whose table
/// repeats each sample over the entries that read it, and remaps the geometry
/// by [`RampMap`] so each entry is read where vello reads its sample. The
/// staircase holds two stops at one offset between steps, which is what makes
/// `vello_gpu` build its widest table.
#[derive(Default)]
struct Staircases {
    /// Every stop list seen recently.
    cache: FxHashMap<StaircaseKey, Staircase>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct StaircaseKey {
    /// Offset and sRGB components of each stop, bit for bit.
    stops: Vec<u32>,
    premultiplied: bool,
    /// The map's bias, which a radial gradient may have moved.
    bias: u64,
    extend: u8,
}

/// One stop list's ramp, and the staircases built from it.
struct Staircase {
    ramp: Vec<PremulRgba8>,
    built: Vec<(Lowered, peniko::ColorStops)>,
}

/// Stop lists cached before the cache is dropped.
const STAIRCASES_RETAINED: usize = 256;

impl Staircases {
    fn gradient(
        &mut self,
        geometry: RampGeometry,
        stops: &[peniko::ColorStop],
        extend: Extend,
        alpha_space: peniko::InterpolationAlphaSpace,
        draw: Option<&LinearDraw>,
    ) -> (Gradient, Option<Split>) {
        let (mut gradient, map) = remap(geometry, RampMap::new(extend));
        gradient.extend = extend;
        gradient.interpolation_alpha_space = peniko::InterpolationAlphaSpace::Premultiplied;
        let key = StaircaseKey {
            stops: stops
                .iter()
                .flat_map(|stop| {
                    let [r, g, b, a] = stop.color.to_alpha_color::<Srgb>().components;
                    [
                        stop.offset.to_bits(),
                        r.to_bits(),
                        g.to_bits(),
                        b.to_bits(),
                        a.to_bits(),
                    ]
                })
                .collect(),
            premultiplied: alpha_space == peniko::InterpolationAlphaSpace::Premultiplied,
            bias: map.bias.to_bits(),
            extend: extend as u8,
        };
        if !self.cache.contains_key(&key) && self.cache.len() >= STAIRCASES_RETAINED {
            self.cache.clear();
        }
        let entry = self.cache.entry(key).or_insert_with(|| Staircase {
            ramp: vello_ramp(stops, alpha_space),
            built: Vec::new(),
        });
        let ties = draw.map_or_else(Ties::default, |draw| ties(draw, &entry.ramp, extend));
        let mut stairs = |lowered: Lowered| {
            if let Some((_, stairs)) = entry.built.iter().find(|(built, _)| *built == lowered) {
                return stairs.clone();
            }
            let stairs = staircase(&entry.ramp, map, &lowered);
            if entry.built.len() >= 8 {
                entry.built.clear();
            }
            entry.built.push((lowered, stairs.clone()));
            stairs
        };
        let split = (!ties.minority.is_empty()).then(|| {
            let mut alt = gradient.clone();
            let mut lowered = ties.lowered;
            for (bits, split) in lowered.iter_mut().zip(ties.split) {
                *bits ^= split;
            }
            alt.stops = stairs(lowered);
            Split {
                alt: Some(alt.into()),
                pixels: ties.minority,
            }
        });
        gradient.stops = stairs(ties.lowered);
        (gradient, split)
    }
}

/// The table `map` reads from `ramp`, as stops: each entry reads the sample
/// vello reads there, and an entry a boundary crosses reads the upper one
/// unless `lowered` sends the boundary's ties below.
fn staircase(ramp: &[PremulRgba8], map: RampMap, lowered: &Lowered) -> peniko::ColorStops {
    let read = |k: usize| {
        let (lo, hi) = map.samples(k);
        if lo != hi && lowered[hi / 64] & (1 << (hi % 64)) != 0 {
            lo
        } else {
            hi
        }
    };
    let mut stairs = peniko::ColorStops::default();
    let mut start = 0_usize;
    while start < LUT_WIDTH {
        let color = ramp[read(start)];
        let mut end = start + 1;
        while end < LUT_WIDTH && ramp[read(end)] == color {
            end += 1;
        }
        let color = stair_color(color);
        stairs.push((start as f32 / LUT_WIDTH as f32, color).into());
        stairs.push((end as f32 / LUT_WIDTH as f32, color).into());
        start = end;
    }
    stairs
}

/// The pixel of a sweep gradient vello leaves unpainted: the one whose
/// top-left corner is the gradient's centre. There vello's angle is
/// `atan(0 / 0)`, and the ramp read at the resulting index draws nothing;
/// `vello_gpu` reads its first entry instead. Only a centre on an exact
/// corner under an axis-aligned brush evaluates to that zero.
fn sweep_centre(transform: &Transform, center: Point) -> Option<Split> {
    let [a, b, c, d] = transform.matrix;
    let [e, f] = transform.translation;
    if b != 0.0 || c != 0.0 {
        return None;
    }
    let x = a.mul_add(center.x as f32, e);
    let y = d.mul_add(center.y as f32, f);
    (x.fract() == 0.0 && y.fract() == 0.0).then(|| Split {
        alt: None,
        pixels: vec![Rect::new(
            f64::from(x),
            f64::from(y),
            f64::from(x) + 1.0,
            f64::from(y) + 1.0,
        )],
    })
}

/// Pixels a draw paints otherwise than the rest of it, because vello's
/// sample there is decided per pixel: drawn from `alt`, or with `None` not at
/// all.
struct Split {
    alt: Option<vello_gpu::PaintType>,
    /// Pixel-aligned device rectangles.
    pixels: Vec<Rect>,
}

/// Runs `draw` with `paint` over every pixel outside `split`, then with the
/// split's own paint over its pixels. The clips are pixel-aligned, so each
/// pixel's coverage comes from one of the two draws, whole. `transform` and
/// `fill` are the scene's, restored around each clip push.
fn draw_split(
    scene: &mut Scene,
    split: Option<Split>,
    paint: vello_gpu::PaintType,
    transform: Affine,
    fill: Fill,
    (width, height): (u16, u16),
    mut draw: impl FnMut(&mut Scene, vello_gpu::PaintType),
) {
    let Some(split) = split else {
        draw(scene, paint);
        return;
    };
    let clip = |scene: &mut Scene, path: &BezPath, rule: Fill| {
        scene.set_transform(Affine::IDENTITY);
        scene.set_fill_rule(rule);
        scene.push_clip_path(path);
        scene.set_transform(transform);
        scene.set_fill_rule(fill);
    };
    let mut pixels = BezPath::new();
    for rect in &split.pixels {
        pixels.extend(rect.path_elements(0.0));
    }
    let mut outside =
        Rect::new(-1.0, -1.0, f64::from(width) + 1.0, f64::from(height) + 1.0).to_path(0.0);
    outside.extend(pixels.iter());
    clip(scene, &outside, Fill::EvenOdd);
    draw(scene, paint);
    scene.pop_clip();
    if let Some(alt) = split.alt {
        clip(scene, &pixels, Fill::NonZero);
        draw(scene, alt);
        scene.pop_clip();
    }
}

/// `geometry` as a `vello_gpu` gradient whose parameter is `map` of vello's,
/// with the map it could honour: a radial gradient keeps both radii
/// non-negative, so a shift that would make one negative is taken a whole
/// period further (repeat) or dropped.
fn remap(geometry: RampGeometry, mut map: RampMap) -> (Gradient, RampMap) {
    let gradient = match geometry {
        RampGeometry::Linear(p0, p1) => {
            // `vello_gpu` adds its own constant to a linear parameter.
            let shifted = RampMap {
                bias: map.bias - LINEAR_SHADER_BIAS,
                ..map
            };
            let d = p1 - p0;
            Gradient::new_linear(p0 + d * shifted.source(0.0), p0 + d * shifted.source(1.0))
        }
        RampGeometry::Radial { c0, r0, c1, r1 } => {
            let radius = |map: RampMap, u: f64| r0 + (r1 - r0) * map.source(u);
            let negative = |map: RampMap| radius(map, 0.0) < 0.0 || radius(map, 1.0) < 0.0;
            if negative(map) && map.extend == Extend::Repeat {
                map.bias -= 1.0;
            }
            if negative(map) {
                map.bias = 0.0;
            }
            let center = |u: f64| c0 + (c1 - c0) * map.source(u);
            Gradient::new_two_point_radial(
                center(0.0),
                radius(map, 0.0) as f32,
                center(1.0),
                radius(map, 1.0) as f32,
            )
        }
        RampGeometry::Sweep { center, t0, t1 } => {
            let turn = |u: f64| (t0 + (t1 - t0) * map.source(u)) * std::f64::consts::TAU;
            Gradient::new_sweep(center, turn(0.0) as f32, turn(1.0) as f32)
        }
    };
    (gradient, map)
}

/// vello's ramp for `stops`: `vello_encoding`'s `make_ramp`, step for step.
fn vello_ramp(
    stops: &[peniko::ColorStop],
    alpha_space: peniko::InterpolationAlphaSpace,
) -> Vec<PremulRgba8> {
    let mut last_u = 0.0_f32;
    let mut last_c = stops[0].color.to_alpha_color::<Srgb>();
    let mut this_u = last_u;
    let mut this_c = last_c;
    let mut j = 0;
    (0..RAMP_SAMPLES)
        .map(|i| {
            let u = (i as f32) / (RAMP_SAMPLES - 1) as f32;
            while u > this_u {
                last_u = this_u;
                last_c = this_c;
                if let Some(s) = stops.get(j + 1) {
                    this_u = s.offset;
                    this_c = s.color.to_alpha_color::<Srgb>();
                    j += 1;
                } else {
                    break;
                }
            }
            let du = this_u - last_u;
            let c = if du < 1e-9 {
                this_c
            } else {
                let t = (u - last_u) / du;
                match alpha_space {
                    peniko::InterpolationAlphaSpace::Premultiplied => {
                        last_c.lerp(this_c, t, HueDirection::default())
                    }
                    peniko::InterpolationAlphaSpace::Unpremultiplied => {
                        last_c + (this_c - last_c) * t
                    }
                }
            };
            c.premultiply().to_rgba8()
        })
        .collect()
}

/// The stop colour whose premultiplied table entry is exactly `rgba`.
fn stair_color(rgba: PremulRgba8) -> Color {
    if rgba.a == 0 {
        return Color::TRANSPARENT;
    }
    let alpha = f32::from(rgba.a);
    Color::new([
        f32::from(rgba.r) / alpha,
        f32::from(rgba.g) / alpha,
        f32::from(rgba.b) / alpha,
        alpha / 255.0,
    ])
}

fn premul_color(rgba: u32) -> Color {
    let [r, g, b, a] = rgba.to_ne_bytes();
    if a == 0 {
        return Color::TRANSPARENT;
    }
    let un = |c: u8| ((f32::from(c) * 255.0 / f32::from(a)).round().min(255.0)) as u8;
    Color::from_rgba8(un(r), un(g), un(b), a)
}

fn extend(bits: u32) -> Extend {
    match bits & 0x3 {
        1 => Extend::Repeat,
        2 => Extend::Reflect,
        _ => Extend::Pad,
    }
}

fn mix(value: u32) -> Mix {
    match value {
        1 => Mix::Multiply,
        2 => Mix::Screen,
        3 => Mix::Overlay,
        4 => Mix::Darken,
        5 => Mix::Lighten,
        6 => Mix::ColorDodge,
        7 => Mix::ColorBurn,
        8 => Mix::HardLight,
        9 => Mix::SoftLight,
        10 => Mix::Difference,
        11 => Mix::Exclusion,
        12 => Mix::Hue,
        13 => Mix::Saturation,
        14 => Mix::Color,
        15 => Mix::Luminosity,
        _ => Mix::Normal,
    }
}

fn compose(value: u32) -> Compose {
    match value {
        0 => Compose::Clear,
        1 => Compose::Copy,
        2 => Compose::Dest,
        4 => Compose::DestOver,
        5 => Compose::SrcIn,
        6 => Compose::DestIn,
        7 => Compose::SrcOut,
        8 => Compose::DestOut,
        9 => Compose::SrcAtop,
        10 => Compose::DestAtop,
        11 => Compose::Xor,
        12 => Compose::Plus,
        13 => Compose::PlusLighter,
        _ => Compose::SrcOver,
    }
}

/// `image`'s pixels as the premultiplied RGBA8 a pixmap holds.
fn pixmap(image: &ImageData) -> Option<Pixmap> {
    let bytes = image.data.data();
    let (width, height) = (image.width, image.height);
    if bytes.len() < (width * height * 4) as usize || width > u32::from(u16::MAX) {
        return None;
    }
    let mut data = bytes[..(width * height * 4) as usize].to_vec();
    for px in data.chunks_exact_mut(4) {
        if image.format == ImageFormat::Bgra8 {
            px.swap(0, 2);
        }
        if image.alpha_type == ImageAlphaType::Alpha {
            let a = u16::from(px[3]);
            for c in &mut px[..3] {
                *c = ((u16::from(*c) * a + 127) / 255) as u8;
            }
        }
    }
    Some(Pixmap::from_parts(
        data,
        width as u16,
        height as u16,
        PixelMetadata::default(),
    ))
}

/// The sampler a `DrawImage`'s packed sample word names.
fn sampler(sample: u32) -> ImageSampler {
    let quality = match (sample >> 12) & 0x3 {
        0 => ImageQuality::Low,
        1 => ImageQuality::Medium,
        _ => ImageQuality::High,
    };
    ImageSampler {
        x_extend: extend(sample >> 10),
        y_extend: extend(sample >> 8),
        quality,
        alpha: (sample & 0xff) as f32 / 255.0,
    }
}

/// The paint drawing `image`: its bound texture when it has one, else its
/// pixels, uploaded once per blob.
#[expect(
    clippy::too_many_arguments,
    reason = "the upload's device handles beside the two caches it consults"
)]
fn image_paint(
    images: &mut FxHashMap<(u64, u32, u32), ImageId>,
    bound: &FxHashMap<u64, Bound>,
    renderer: &mut Renderer,
    resources: &mut Resources,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    image: &ImageData,
    sample: u32,
) -> Option<Image> {
    if let Some(texture) = bound.get(&image.data.id()) {
        return Some(Image {
            image: ImageSource::ExternalTexture {
                id: texture.id,
                source_region: RectU16::new(0, 0, texture.width, texture.height),
                may_have_transparency: true,
            },
            sampler: sampler(sample),
        });
    }
    let key = (image.data.id(), image.width, image.height);
    let id = if let Some(id) = images.get(&key) {
        *id
    } else {
        let pixmap = pixmap(image)?;
        let id = renderer.upload_image(resources, device, queue, encoder, &pixmap);
        images.insert(key, id);
        id
    };
    Some(Image {
        image: ImageSource::opaque_id(id),
        sampler: sampler(sample),
    })
}

impl SparseBackend {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        // The target size here only seeds a config buffer; every render
        // names its own.
        let (renderer, resources) = Renderer::new(
            device,
            &RenderTargetConfig {
                format: wgpu::TextureFormat::Rgba8Unorm,
                width: 1,
                height: 1,
            },
        );
        Self {
            renderer,
            resources,
            scene: Scene::new(1, 1),
            images: FxHashMap::default(),
            bound: FxHashMap::default(),
            next_texture: 0,
            staircases: Staircases::default(),
            units_per_em: FxHashMap::default(),
            glyph_prep: glifo::GlyphPrepCache::default(),
            last: Stats::default(),
        }
    }

    /// Binds `texture` to every draw of `handle`, or with `None` unbinds it.
    pub(crate) fn bind(&mut self, handle: &ImageData, texture: Option<wgpu::Texture>) {
        let key = handle.data.id();
        match texture {
            Some(texture) => {
                let id = TextureId(self.next_texture);
                self.next_texture += 1;
                self.bound.insert(
                    key,
                    Bound {
                        id,
                        view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
                        width: texture.width() as u16,
                        height: texture.height() as u16,
                    },
                );
            }
            None => {
                self.bound.remove(&key);
            }
        }
    }

    /// Renders `vello_scene` into `view`, a `width`×`height` RGBA8 target
    /// cleared to `base_color` first.
    #[expect(
        clippy::too_many_arguments,
        reason = "one render's full inputs: the device pair, the scene, and the target with its \
                  size and base colour"
    )]
    pub(crate) fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vello_scene: &vello::Scene,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        base_color: Color,
    ) -> Result<(), String> {
        let (w, h) = (width as u16, height as u16);
        let Self {
            renderer,
            resources,
            scene,
            images,
            bound,
            staircases,
            units_per_em,
            glyph_prep,
            last,
            ..
        } = self;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("terminus vello_gpu frame"),
        });

        let started = Instant::now();
        let mut stats = Stats::default();
        scene.reset_and_resize(w, h);
        let encoding = vello_scene.encoding();
        let paths = decode_paths(encoding);
        stats.decode = started.elapsed();
        let mut paths = paths.into_iter();
        let ramps: FxHashMap<usize, &Patch> = encoding
            .resources
            .patches
            .iter()
            .filter_map(|patch| match patch {
                Patch::Ramp {
                    draw_data_offset, ..
                }
                | Patch::Image {
                    draw_data_offset, ..
                } => Some((*draw_data_offset, patch)),
                Patch::GlyphRun { .. } => None,
            })
            .collect();
        let runs: FxHashMap<usize, usize> = encoding
            .resources
            .glyph_runs
            .iter()
            .enumerate()
            .map(|(index, run)| (run.stream_offsets.draw_tags, index))
            .collect();
        let data = &encoding.draw_data;
        let mut offset = 0_usize;
        let f = |at: usize| f32::from_bits(data[at]);
        for (index, tag) in encoding.draw_tags.iter().enumerate() {
            let size = ((tag.0 >> 2) & 0x7) as usize;
            let at = offset;
            offset += size;
            if *tag == DrawTag::NOP {
                continue;
            }
            let glyph_run = runs.get(&index).copied();
            let decoded = if glyph_run.is_some() {
                None
            } else {
                paths.next()
            };
            // The paint of this draw, and the pixels it paints otherwise.
            let mut split = None;
            let paint: Option<vello_gpu::PaintType> = match *tag {
                DrawTag::COLOR => Some(premul_color(data[at]).into()),
                DrawTag::LINEAR_GRADIENT | DrawTag::RADIAL_GRADIENT | DrawTag::SWEEP_GRADIENT => {
                    let Some(Patch::Ramp {
                        stops,
                        extend,
                        interpolation_alpha_space,
                        ..
                    }) = ramps.get(&at)
                    else {
                        continue;
                    };
                    let p = |i: usize| Point::new(f64::from(f(at + i)), f64::from(f(at + i + 1)));
                    let geometry = match *tag {
                        DrawTag::LINEAR_GRADIENT => RampGeometry::Linear(p(1), p(3)),
                        DrawTag::RADIAL_GRADIENT => RampGeometry::Radial {
                            c0: p(1),
                            r0: f64::from(f(at + 5)),
                            c1: p(3),
                            r1: f64::from(f(at + 6)),
                        },
                        _ => RampGeometry::Sweep {
                            center: p(1),
                            t0: f64::from(f(at + 3)),
                            t1: f64::from(f(at + 4)),
                        },
                    };
                    let viewport = Rect::new(0.0, 0.0, f64::from(width), f64::from(height));
                    // The brush's device transform as vello's stages hold it,
                    // and the device bounds of the draw.
                    let (transform, bounds) = match (glyph_run, &decoded) {
                        (Some(run), _) => {
                            let run = &encoding.resources.glyph_runs[run];
                            (
                                run.transform * run.brush_transform.unwrap_or(Transform::IDENTITY),
                                viewport,
                            )
                        }
                        (None, Some(decoded)) => (
                            f32_transform(decoded.brush_transform),
                            decoded.path.bounding_box().intersect(viewport),
                        ),
                        (None, None) => continue,
                    };
                    let draw = matches!(geometry, RampGeometry::Linear(..)).then(|| LinearDraw {
                        line: VelloLine::new(
                            &transform,
                            [f(at + 1), f(at + 2)],
                            [f(at + 3), f(at + 4)],
                        ),
                        corners: Corners {
                            x0: bounds.x0.floor() as i32,
                            y0: bounds.y0.floor() as i32,
                            x1: bounds.x1.ceil() as i32,
                            y1: bounds.y1.ceil() as i32,
                        },
                    });
                    let (gradient, ties) = staircases.gradient(
                        geometry,
                        &encoding.resources.color_stops[stops.clone()],
                        *extend,
                        *interpolation_alpha_space,
                        draw.as_ref(),
                    );
                    split = match geometry {
                        RampGeometry::Sweep { center, .. } => sweep_centre(&transform, center),
                        _ => ties,
                    };
                    Some(gradient.into())
                }
                DrawTag::IMAGE => {
                    let Some(Patch::Image { image, .. }) = ramps.get(&at) else {
                        continue;
                    };
                    let paint = image_paint(
                        images,
                        bound,
                        renderer,
                        resources,
                        device,
                        queue,
                        &mut encoder,
                        image,
                        data[at + 2],
                    );
                    if paint.is_none() {
                        stats.images_skipped += 1;
                    }
                    paint.map(Into::into)
                }
                DrawTag::BLUR_RECT => Some(premul_color(data[at]).into()),
                _ => None,
            };

            if let Some(run_index) = glyph_run {
                let run = &encoding.resources.glyph_runs[run_index];
                let Some(paint) = paint else { continue };
                stats.glyph_runs += 1;
                let run_transform = affine(&run.transform);
                scene.set_transform(run_transform);
                let brush =
                    run_transform * run.brush_transform.map_or(Affine::IDENTITY, |t| affine(&t));
                let paint_transform = match &paint {
                    vello_gpu::PaintType::Gradient(_) => corner_sampled(run_transform, brush),
                    _ => run_transform.inverse() * brush,
                };
                scene.set_paint_transform(paint_transform);
                let coords = &encoding.resources.normalized_coords[run.normalized_coords.clone()];
                // vello applies the per-glyph transform in the outline's
                // y-up space; glifo in the scene's y-down one.
                let flip = Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, 0.0]);
                let glyph_transform = run.glyph_transform.map(|t| flip * affine(&t) * flip);
                let embolden = embolden(
                    run,
                    run_transform * glyph_transform.unwrap_or(Affine::IDENTITY),
                    units_per_em,
                );
                if let peniko::Style::Stroke(stroke) = &run.style {
                    scene.set_stroke(stroke.clone());
                }
                let fill = Fill::NonZero;
                scene.set_fill_rule(fill);
                draw_split(
                    scene,
                    split,
                    paint,
                    run_transform,
                    fill,
                    (w, h),
                    |scene, paint| {
                        scene.set_paint(paint);
                        let run_glyphs =
                            encoding.resources.glyphs[run.glyphs.clone()]
                                .iter()
                                .map(|g| glifo::Glyph {
                                    id: g.id,
                                    x: g.x,
                                    y: g.y,
                                });
                        let backend = glyphs::Backend {
                            glyphs: glyphs::Glyphs::new(scene),
                            prep: glyph_prep,
                        };
                        let mut builder = glifo::GlyphRunBuilder::new(
                            run.font.clone(),
                            run_transform,
                            paint_transform,
                            backend,
                        )
                        .font_size(run.font_size)
                        .hint(run.hint)
                        .normalized_coords(coords)
                        .font_embolden(embolden);
                        if let Some(t) = glyph_transform {
                            builder = builder.glyph_transform(t);
                        }
                        // A glyph the font cannot outline draws nothing, as in vello.
                        let _ = match &run.style {
                            peniko::Style::Fill(_) => builder.fill_glyphs(run_glyphs),
                            peniko::Style::Stroke(_) => builder.stroke_glyphs(run_glyphs),
                        };
                    },
                );
                continue;
            }

            let Some(decoded) = decoded else { continue };
            // The path is already in device space.
            scene.set_transform(Affine::IDENTITY);
            scene.set_fill_rule(decoded.fill());
            match *tag {
                DrawTag::BEGIN_CLIP => {
                    stats.layers += 1;
                    let blend = data[at];
                    let alpha = f(at + 1);
                    if blend == DrawBeginClip::CLIP_BLEND_MODE {
                        scene.push_clip_layer(&decoded.path);
                    } else {
                        if blend == DrawBeginClip::LUMINANCE_MASK_BLEND_MODE {
                            stats.masks_skipped += 1;
                        }
                        let mode = BlendMode::new(mix((blend >> 8) & 0xff), compose(blend & 0xff));
                        scene.push_layer(Some(&decoded.path), Some(mode), Some(alpha), None, None);
                    }
                }
                DrawTag::END_CLIP => scene.pop_layer(),
                DrawTag::BLUR_RECT => {
                    stats.draws += 1;
                    if let Some(paint) = paint {
                        scene.set_paint(paint);
                    }
                    scene.set_transform(decoded.brush_transform);
                    scene.set_paint_transform(corner_sampled(
                        decoded.brush_transform,
                        decoded.brush_transform,
                    ));
                    let (width, height) = (f64::from(f(at + 1)), f64::from(f(at + 2)));
                    scene.fill_blurred_rounded_rect(
                        &Rect::from_center_size(Point::ORIGIN, (width, height)),
                        f(at + 3),
                        f(at + 4),
                        false,
                    );
                }
                _ => {
                    let Some(paint) = paint else { continue };
                    stats.draws += 1;
                    match &paint {
                        vello_gpu::PaintType::Solid(_) => scene.reset_paint_transform(),
                        vello_gpu::PaintType::Gradient(_) => scene.set_paint_transform(
                            corner_sampled(Affine::IDENTITY, decoded.brush_transform),
                        ),
                        vello_gpu::PaintType::Image(_) => {
                            scene.set_paint_transform(decoded.brush_transform);
                        }
                    }
                    draw_split(
                        scene,
                        split,
                        paint,
                        Affine::IDENTITY,
                        decoded.fill(),
                        (w, h),
                        |scene, paint| {
                            scene.set_paint(paint);
                            scene.fill_path(&decoded.path);
                        },
                    );
                }
            }
        }
        stats.encode = started.elapsed();
        glyph_prep.maintain();

        let started = Instant::now();
        let render_size = RenderSize {
            width: w,
            height: h,
        };
        let mut bindings = TextureBindings::new();
        for texture in bound.values() {
            bindings.insert(texture.id, texture.view.clone());
        }
        let result = renderer.render(
            scene,
            resources,
            device,
            queue,
            &mut encoder,
            &render_size,
            view,
            None,
            &bindings,
            TargetInit::Clear(ClearSettings::Viewport {
                color: base_color.convert(),
            }),
        );
        queue.submit([encoder.finish()]);
        stats.render = started.elapsed();
        *last = stats;
        result.map_err(|error| error.to_string())
    }
}
