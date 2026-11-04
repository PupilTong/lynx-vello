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
//! external texture, sampled in place; any other bitmap is uploaded once and
//! released once renders stop drawing it ([`Bitmaps`]).

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
    AtlasWriter, ClearSettings, Image, ImageId, ImageSource, PixelMetadata, Pixmap, RectU16,
    RenderSize, RenderTargetConfig, Renderer, Resources, Scene, TargetInit, TextureBindings,
    TextureId,
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
    /// Uploaded bitmaps, with their lifecycle.
    images: Bitmaps,
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

/// Whether some layer of `encoding` that the root holds blends by anything
/// but source-over.
///
/// vello starts every pixel of the root at the base colour, so such a layer
/// blends against it. `vello_gpu` renders a root that is a blend target into
/// a transparent intermediate and blits that over the cleared target, so the
/// clear colour is under the blend rather than in it; [`SparseBackend::render`]
/// then draws the base colour as the scene's first fill instead.
fn blends_over_root(encoding: &Encoding) -> bool {
    let mut depth = 0_u32;
    let mut offset = 0_usize;
    for tag in &encoding.draw_tags {
        let at = offset;
        offset += ((tag.0 >> 2) & 0x7) as usize;
        match *tag {
            DrawTag::BEGIN_CLIP => {
                let blend = encoding.draw_data[at];
                if depth == 0
                    && blend != DrawBeginClip::CLIP_BLEND_MODE
                    && BlendMode::new(mix((blend >> 8) & 0xff), compose(blend & 0xff))
                        != BlendMode::default()
                {
                    return true;
                }
                depth += 1;
            }
            DrawTag::END_CLIP => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

/// The bitmap bytes kept resident across renders, in the image atlas and in
/// bitmaps' own textures together.
#[cfg(not(target_arch = "wasm32"))]
const MAX_BITMAP_BYTES: u64 = 128 * 1024 * 1024;
/// The bitmap bytes kept resident across renders, in the image atlas and in
/// bitmaps' own textures together.
#[cfg(target_arch = "wasm32")]
const MAX_BITMAP_BYTES: u64 = 64 * 1024 * 1024;

/// Of those, the bytes that may live in `vello_gpu`'s image atlas at once.
/// The atlas shares its 4096 × 4096 layers with the glyph cache, creates them
/// as it fills and frees none, and `Renderer::upload_image` panics when no
/// layer has room; so bitmaps take at most one layer's worth of it and the
/// rest take their own textures.
const MAX_ATLAS_BITMAP_BYTES: u64 = 64 * 1024 * 1024;

/// The longest side, edge included, a bitmap is uploaded to the atlas with.
/// A longer one is a draw of its own however it is stored, and in the atlas
/// it would hold a large share of a layer that is never freed; its own
/// texture goes when it does.
const MAX_ATLAS_BITMAP_SIDE: u16 = 1024;

/// How far a nearest-neighbour sample position is moved along each image
/// axis, in texels.
///
/// `vello_gpu` forms an image sample position at the corners of a strip's
/// quad and the rasterizer interpolates it, so a pixel centre that maps
/// exactly onto a texel boundary comes out an ulp or two to either side of
/// it. vello evaluates the position per pixel, and the exact value is on the
/// boundary, which nearest-neighbour sampling gives to the texel after it.
/// 2^-12 texel is several ulps of any texel coordinate below 512, and far
/// below the distance a pixel centre that is not on a boundary keeps from one
/// at any ordinary scale.
const TIE_NUDGE: f64 = 1.0 / 4096.0;

/// One bitmap upload's identity: the pixels, and which axes carry the
/// repeated edge [`Bitmaps`] describes.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct BitmapKey {
    blob: u64,
    width: u32,
    height: u32,
    /// [`ImageFormat`] and [`ImageAlphaType`] by their `repr(u8)`: one blob
    /// read under another layout is other pixels.
    layout: (u8, u8),
    edge: (bool, bool),
}

/// Where an uploaded bitmap lives.
enum Store {
    Atlas(ImageId),
    /// A texture of its own, drawn as an external texture.
    Texture {
        id: TextureId,
        view: wgpu::TextureView,
    },
}

/// One uploaded bitmap.
struct Bitmap {
    store: Store,
    /// The uploaded size, the repeated edge included.
    width: u16,
    height: u16,
    opaque: bool,
    bytes: u64,
    /// The render that last drew it; see [`Bitmaps::tick`].
    last_used: u64,
}

impl Bitmap {
    fn image(&self, sampler: ImageSampler) -> Image {
        let image = match &self.store {
            Store::Atlas(id) => ImageSource::opaque_id_with_transparency_hint(*id, !self.opaque),
            Store::Texture { id, .. } => ImageSource::ExternalTexture {
                id: *id,
                source_region: RectU16::new(0, 0, self.width, self.height),
                may_have_transparency: !self.opaque,
            },
        };
        Image { image, sampler }
    }
}

/// How many of the latest bitmap-drawing renders pin what they drew. A frame
/// is several renders through one backend — its `filter: blur()` and vector
/// bakes, then the composite — so what the composite drew stays pinned
/// through the next frame's bakes; vello's own image cache spares what the
/// last two of its renders drew the same way. A render that draws no bitmap
/// (most bakes) is not a tick at all.
const PINNED_RENDERS: u64 = 4;

/// The bitmaps a render draws, uploaded once per [`BitmapKey`] and released
/// once no render draws them.
///
/// # Upload
///
/// A bitmap is converted to the premultiplied RGBA8 `vello_gpu` samples
/// ([`pixmap`]) and uploaded to the shared image atlas, or, when it is longer
/// than [`MAX_ATLAS_BITMAP_SIDE`] or the atlas already holds
/// [`MAX_ATLAS_BITMAP_BYTES`] of bitmaps, to a texture of its own drawn as an
/// external texture. The key names the blob, its size and its layout, so a
/// blob drawn at another size or under another layout is another upload, and
/// pixels that change take a new blob and so a new key; the old upload ages
/// out below.
///
/// `vello_gpu` pads an image axis by clamping the sample position to the last
/// texel's *centre* before it filters, where vello clamps it to the image's
/// edge: a bilinear or bicubic sample past the last centre blends the last
/// two texels half and half — a half-texel band along the right and bottom
/// edge of every magnified bitmap, and the whole padded area beyond it. So an
/// axis drawn with [`Extend::Pad`] is uploaded with its last column (row)
/// repeated once more: the clamp then lands on the repeated texel's centre,
/// the image's own edge, and every filter reads what vello reads. A repeating
/// axis keeps its exact period and is uploaded as it is.
///
/// # Lifecycle
///
/// Every render that draws a bitmap is one tick, and what the last
/// [`PINNED_RENDERS`] of them drew is pinned. Once a render's draws are
/// encoded, the unpinned uploads are released least recently drawn first
/// until the resident bytes are back within [`MAX_BITMAP_BYTES`] — an atlas
/// upload with `Renderer::destroy_image`, a texture by dropping it. Renders
/// whose own bitmaps exceed the budget keep them all while they are pinned:
/// the bound is on what is kept across renders, and resident bytes never
/// exceed the larger of the budget and what the pinned renders drew.
pub(crate) struct Bitmaps {
    entries: FxHashMap<BitmapKey, Bitmap>,
    /// Bytes of every upload, and of those in the atlas.
    bytes: u64,
    atlas_bytes: u64,
    /// How many renders have drawn a bitmap. An upload's `last_used` is the
    /// tick of the render that last drew it, so those within
    /// [`PINNED_RENDERS`] of the current tick are the pinned ones and the
    /// smallest tick is the next to go.
    tick: u64,
    /// Whether the current render has drawn a bitmap, and so is a tick.
    drawing: bool,
    budget: u64,
    /// The device's longest texture side, read at the first upload.
    longest: Option<u32>,
}

impl Bitmaps {
    fn with_budget(budget: u64) -> Self {
        Self {
            entries: FxHashMap::default(),
            bytes: 0,
            atlas_bytes: 0,
            tick: 0,
            drawing: false,
            budget,
            longest: None,
        }
    }

    /// Starts a render; its first bitmap draw makes it a tick.
    fn begin(&mut self) {
        self.drawing = false;
    }

    /// The paint drawing `image` under `sampler`, uploaded on a miss; `None`
    /// when the image's pixels are unreadable.
    #[expect(
        clippy::too_many_arguments,
        reason = "an upload's device handles beside the texture ids it draws from"
    )]
    fn paint(
        &mut self,
        renderer: &mut Renderer,
        resources: &mut Resources,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        next_texture: &mut u64,
        image: &ImageData,
        sampler: ImageSampler,
    ) -> Option<Image> {
        // The repeated edge needs one texel more than the image: a texture
        // already as long as the device allows keeps the shader's clamp.
        let longest = *self
            .longest
            .get_or_insert_with(|| device.limits().max_texture_dimension_2d);
        let key = BitmapKey {
            blob: image.data.id(),
            width: image.width,
            height: image.height,
            layout: (image.format as u8, image.alpha_type as u8),
            edge: (
                sampler.x_extend == Extend::Pad && image.width < longest,
                sampler.y_extend == Extend::Pad && image.height < longest,
            ),
        };
        if !self.drawing {
            self.drawing = true;
            self.tick += 1;
        }
        let tick = self.tick;
        if let Some(bitmap) = self.entries.get_mut(&key) {
            bitmap.last_used = tick;
            return Some(bitmap.image(sampler));
        }
        let (pixmap, opaque) = pixmap(image, key.edge)?;
        let (width, height) = (pixmap.width(), pixmap.height());
        let bytes = u64::from(width) * u64::from(height) * 4;
        let store = if width.max(height) <= MAX_ATLAS_BITMAP_SIDE
            && self.atlas_bytes + bytes <= MAX_ATLAS_BITMAP_BYTES
        {
            self.atlas_bytes += bytes;
            Store::Atlas(renderer.upload_image(resources, device, queue, encoder, &pixmap))
        } else {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("terminus vello_gpu bitmap"),
                size: wgpu::Extent3d {
                    width: u32::from(width),
                    height: u32::from(height),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            pixmap.write_to_atlas(device, queue, encoder, &texture, [0, 0], width, height);
            let id = TextureId(*next_texture);
            *next_texture += 1;
            Store::Texture {
                id,
                view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
            }
        };
        self.bytes += bytes;
        let bitmap = self.entries.entry(key).or_insert(Bitmap {
            store,
            width,
            height,
            opaque,
            bytes,
            last_used: tick,
        });
        Some(bitmap.image(sampler))
    }

    /// Binds the textures of the bitmaps the current render draws from their
    /// own texture.
    fn bind_into(&self, bindings: &mut TextureBindings) {
        for bitmap in self.entries.values() {
            if let Store::Texture { id, view } = &bitmap.store
                && self.drawing
                && bitmap.last_used == self.tick
            {
                bindings.insert(*id, view.clone());
            }
        }
    }

    /// Ends a render: releases the unpinned uploads, least recently drawn
    /// first, until the resident bytes are within the budget.
    ///
    /// Called once the render's draws are encoded into `encoder` and before
    /// it is submitted. `destroy_image` clears the freed atlas region with a
    /// pass in `encoder`, so the clear runs after every draw that sampled the
    /// region, and an upload that lands there later — a queued write, which
    /// applies at the start of the submit after this one — after the clear.
    /// Releasing inside the render instead would let this render's own
    /// uploads, applied before its encoder runs, be cleared by it.
    fn finish(
        &mut self,
        renderer: &mut Renderer,
        resources: &mut Resources,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        while self.bytes > self.budget {
            let victim = self
                .entries
                .iter()
                .filter(|(_, bitmap)| bitmap.last_used + PINNED_RENDERS <= self.tick)
                .min_by_key(|(_, bitmap)| bitmap.last_used)
                .map(|(key, _)| *key);
            let Some(key) = victim else {
                break;
            };
            let bitmap = self
                .entries
                .remove(&key)
                .expect("the victim was just found in the table");
            self.bytes -= bitmap.bytes;
            if let Store::Atlas(id) = bitmap.store {
                self.atlas_bytes -= bitmap.bytes;
                renderer.destroy_image(resources, encoder, id);
            }
        }
    }
}

/// `image`'s pixels as the premultiplied RGBA8 a pixmap holds, with the last
/// column and the last row repeated once more on the axes `edge` names (see
/// [`Bitmaps`]), and whether every pixel is opaque.
fn pixmap(image: &ImageData, edge: (bool, bool)) -> Option<(Pixmap, bool)> {
    let bytes = image.data.data();
    let (width, height) = (image.width as usize, image.height as usize);
    let row = width * 4;
    let padded_width = width + usize::from(edge.0);
    let padded_height = height + usize::from(edge.1);
    let (Ok(pixmap_width), Ok(pixmap_height)) =
        (u16::try_from(padded_width), u16::try_from(padded_height))
    else {
        return None;
    };
    if width == 0 || height == 0 || bytes.len() < row * height {
        return None;
    }
    let mut data = Vec::with_capacity(padded_width * padded_height * 4);
    let mut opaque = true;
    for source in bytes[..row * height].chunks_exact(row) {
        for px in source.chunks_exact(4) {
            let [mut r, g, mut b, a] = [px[0], px[1], px[2], px[3]];
            if image.format == ImageFormat::Bgra8 {
                std::mem::swap(&mut r, &mut b);
            }
            opaque &= a == 255;
            let mut texel = [r, g, b, a];
            if image.alpha_type == ImageAlphaType::Alpha {
                let a = u16::from(a);
                for c in &mut texel[..3] {
                    *c = ((u16::from(*c) * a + 127) / 255) as u8;
                }
            }
            data.extend_from_slice(&texel);
        }
        if edge.0 {
            data.extend_from_within(data.len() - 4..);
        }
    }
    if edge.1 {
        data.extend_from_within(data.len() - padded_width * 4..);
    }
    let metadata = PixelMetadata::new(ImageAlphaType::AlphaPremultiplied, !opaque);
    Some((
        Pixmap::from_parts(data, pixmap_width, pixmap_height, metadata),
        opaque,
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

/// The paint drawing `image`, and the image-space map to compose onto the
/// right of its paint transform: its bound texture when it has one, else its
/// pixels ([`Bitmaps`]); the map is [`TIE_NUDGE`]'s, for a nearest-neighbour
/// draw.
#[expect(
    clippy::too_many_arguments,
    reason = "the upload's device handles beside the two caches it consults"
)]
fn image_paint(
    images: &mut Bitmaps,
    bound: &FxHashMap<u64, Bound>,
    renderer: &mut Renderer,
    resources: &mut Resources,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    next_texture: &mut u64,
    image: &ImageData,
    sample: u32,
) -> Option<(Image, Affine)> {
    let sampler = sampler(sample);
    // Composed on the right, the map moves the image by −nudge in its own
    // space, so every sample position by +nudge.
    let nudge = if sampler.quality == ImageQuality::Low {
        Affine::translate((-TIE_NUDGE, -TIE_NUDGE))
    } else {
        Affine::IDENTITY
    };
    if let Some(texture) = bound.get(&image.data.id()) {
        // A texture longer than its handle carries the repeated edge a vector
        // bake adds (`vector_textures`), sampled on a padded axis only.
        let region = |extend: Extend, size: u32, texture: u16| {
            let size = u16::try_from(size).map_or(texture, |size| size.min(texture));
            if extend == Extend::Pad && texture > size {
                size + 1
            } else {
                size
            }
        };
        let paint = Image {
            image: ImageSource::ExternalTexture {
                id: texture.id,
                source_region: RectU16::new(
                    0,
                    0,
                    region(sampler.x_extend, image.width, texture.width),
                    region(sampler.y_extend, image.height, texture.height),
                ),
                may_have_transparency: true,
            },
            sampler,
        };
        return Some((paint, nudge));
    }
    let paint = images.paint(
        renderer,
        resources,
        device,
        queue,
        encoder,
        next_texture,
        image,
        sampler,
    )?;
    Some((paint, nudge))
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
            images: Bitmaps::with_budget(MAX_BITMAP_BYTES),
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
            next_texture,
            staircases,
            units_per_em,
            glyph_prep,
            last,
        } = self;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("terminus vello_gpu frame"),
        });
        images.begin();

        let started = Instant::now();
        let mut stats = Stats::default();
        scene.reset_and_resize(w, h);
        let encoding = vello_scene.encoding();
        // See `blends_over_root`: the base colour as a fill is what a root
        // blend has to read, and over a transparent clear it composes to the
        // same pixels the clear would have.
        let underlay = base_color.components[3] > 0.0 && blends_over_root(encoding);
        if underlay {
            scene.set_transform(Affine::IDENTITY);
            scene.set_fill_rule(Fill::NonZero);
            scene.set_paint(base_color);
            scene.reset_paint_transform();
            scene.fill_rect(&Rect::new(0.0, 0.0, f64::from(w), f64::from(h)));
        }
        let clear = if underlay {
            Color::TRANSPARENT
        } else {
            base_color
        };
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
            // An image paint's own image-space map (`image_paint`).
            let mut nudge = Affine::IDENTITY;
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
                        next_texture,
                        image,
                        data[at + 2],
                    );
                    if paint.is_none() {
                        stats.images_skipped += 1;
                    }
                    paint.map(|(paint, map)| {
                        nudge = map;
                        paint.into()
                    })
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
                            scene.set_paint_transform(decoded.brush_transform * nudge);
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
        images.bind_into(&mut bindings);
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
                color: clear.convert(),
            }),
        );
        images.finish(renderer, resources, &mut encoder);
        queue.submit([encoder.finish()]);
        stats.render = started.elapsed();
        *last = stats;
        result.map_err(|error| error.to_string())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Arc;

    use vello::kurbo::{Affine, Rect};
    use vello::peniko::{
        Blob, Color, Extend, Fill, ImageAlphaType, ImageBrush, ImageData, ImageFormat,
        ImageQuality, ImageSampler,
    };
    use vello::util::RenderContext;
    use vello::wgpu;

    use super::{PINNED_RENDERS, SparseBackend, Store};

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    /// A device, a backend on it, and one `width` × `height` target read back
    /// after each render.
    struct Rig {
        context: RenderContext,
        device: usize,
        backend: SparseBackend,
        target: wgpu::Texture,
        width: u32,
        height: u32,
    }

    impl Rig {
        fn new(width: u32, height: u32) -> Self {
            let mut context = RenderContext::new();
            let device = pollster::block_on(context.device(None))
                .expect("a usable GPU adapter is mandatory");
            let handle = &context.devices[device];
            let backend = SparseBackend::new(&handle.device);
            let target = handle.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("sparse test target"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            Self {
                context,
                device,
                backend,
                target,
                width,
                height,
            }
        }

        /// Renders `scene` over white and answers the target's pixel at
        /// each of `at`.
        fn render(&mut self, scene: &vello::Scene) -> Pixels {
            let handle = &self.context.devices[self.device];
            let (device, queue) = (&handle.device, &handle.queue);
            let view = self
                .target
                .create_view(&wgpu::TextureViewDescriptor::default());
            self.backend
                .render(
                    device,
                    queue,
                    scene,
                    &view,
                    self.width,
                    self.height,
                    Color::WHITE,
                )
                .expect("render");
            let row = self.width * 4;
            let padded = row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
                * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("sparse test readback"),
                size: u64::from(padded * self.height),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("sparse test readback copy"),
            });
            encoder.copy_texture_to_buffer(
                self.target.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded),
                        rows_per_image: None,
                    },
                },
                wgpu::Extent3d {
                    width: self.width,
                    height: self.height,
                    depth_or_array_layers: 1,
                },
            );
            queue.submit([encoder.finish()]);
            let slice = buffer.slice(..);
            let (sender, receiver) = flume::bounded(1);
            slice.map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("poll");
            receiver.recv().expect("map callback").expect("map");
            let mapped = slice.get_mapped_range().expect("mapped range");
            Pixels {
                width: self.width,
                rgba: mapped
                    .chunks_exact(padded as usize)
                    .flat_map(|line| line[..row as usize].iter().copied())
                    .collect(),
            }
        }
    }

    /// One read-back target.
    struct Pixels {
        width: u32,
        rgba: Vec<u8>,
    }

    impl Pixels {
        fn at(&self, x: u32, y: u32) -> [u8; 4] {
            let at = ((y * self.width + x) * 4) as usize;
            self.rgba[at..at + 4].try_into().expect("four channels")
        }
    }

    /// A straight-alpha RGBA8 bitmap of `texels`, `width` wide.
    fn bitmap(width: u32, texels: &[[u8; 4]]) -> ImageData {
        let bytes: Vec<u8> = texels.iter().flatten().copied().collect();
        ImageData {
            data: Blob::new(Arc::new(bytes)),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width,
            height: u32::try_from(texels.len()).expect("a small bitmap") / width,
        }
    }

    /// Fills `rect` with `image`, placed by `brush`.
    fn draw(
        scene: &mut vello::Scene,
        image: &ImageData,
        quality: ImageQuality,
        extend: Extend,
        brush: Affine,
        rect: Rect,
    ) {
        let brush_ref = ImageBrush {
            image,
            sampler: ImageSampler {
                x_extend: extend,
                y_extend: extend,
                quality,
                alpha: 1.0,
            },
        };
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            brush_ref,
            Some(brush),
            &rect,
        );
    }

    /// A bilinear draw padded past its last texel's centre reads that texel,
    /// as vello does — not the half-and-half blend of the last two that
    /// `vello_gpu`'s own clamp gives — and a repeating one keeps its period.
    #[test]
    fn a_padded_axis_filters_to_the_last_texel_and_a_repeating_one_keeps_its_period() {
        let mut rig = Rig::new(64, 16);
        let image = bitmap(2, &[RED, BLUE]);
        let mut scene = vello::Scene::new();
        // Two texels at 8 px each, padded out to 32 px.
        draw(
            &mut scene,
            &image,
            ImageQuality::Medium,
            Extend::Pad,
            Affine::scale(8.0),
            Rect::new(0.0, 0.0, 32.0, 8.0),
        );
        // The same two texels repeated across 32 px below.
        draw(
            &mut scene,
            &image,
            ImageQuality::Medium,
            Extend::Repeat,
            Affine::translate((0.0, 8.0)) * Affine::scale(8.0),
            Rect::new(0.0, 8.0, 32.0, 16.0),
        );
        let pixels = rig.render(&scene);
        assert_eq!(pixels.at(1, 4), RED, "before the first texel's centre");
        assert_eq!(pixels.at(14, 4), BLUE, "past the last texel's centre");
        assert_eq!(pixels.at(28, 4), BLUE, "in the padded area");
        // Between the centres the filter mixes the two: pixel 7's centre is
        // 7.5 / 8 texels in, 0.4375 of the way from the first centre.
        let mid = pixels.at(7, 4);
        assert!(
            (i32::from(mid[0]) - 143).abs() <= 2 && (i32::from(mid[2]) - 112).abs() <= 2,
            "{mid:?}"
        );
        assert_eq!(pixels.at(17, 12), RED, "a repeat starts over");
        assert_eq!(pixels.at(30, 12), BLUE);
        assert_eq!(
            rig.backend.images.entries.len(),
            2,
            "the padded and the repeating draw are two uploads of one blob",
        );
    }

    /// A nearest-neighbour pixel centre exactly on a texel boundary takes the
    /// texel after it, the exact value's.
    #[test]
    fn a_nearest_sample_on_a_texel_boundary_takes_the_texel_after_it() {
        let mut rig = Rig::new(128, 4);
        let image = bitmap(8, &[RED, RED, BLUE, BLUE, BLUE, BLUE, RED, RED]);
        let mut scene = vello::Scene::new();
        // At 13.75 px per texel, pixel 27's centre is exactly texel 2's
        // left edge and pixel 82's texel 6's: an object-fit golden's scale.
        draw(
            &mut scene,
            &image,
            ImageQuality::Low,
            Extend::Pad,
            Affine::scale(13.75),
            Rect::new(0.0, 0.0, 110.0, 4.0),
        );
        let pixels = rig.render(&scene);
        assert_eq!([pixels.at(26, 0), pixels.at(27, 0)], [RED, BLUE]);
        assert_eq!([pixels.at(81, 0), pixels.at(82, 0)], [BLUE, RED]);
    }

    /// One blob drawn as two sizes is two uploads, each its own pixels.
    #[test]
    fn one_blob_at_two_sizes_is_two_uploads() {
        let mut rig = Rig::new(16, 16);
        let square = bitmap(2, &[RED, BLUE, BLUE, RED]);
        let row = ImageData {
            width: 4,
            height: 1,
            ..square.clone()
        };
        let mut scene = vello::Scene::new();
        draw(
            &mut scene,
            &square,
            ImageQuality::Low,
            Extend::Pad,
            Affine::scale(4.0),
            Rect::new(0.0, 0.0, 8.0, 8.0),
        );
        draw(
            &mut scene,
            &row,
            ImageQuality::Low,
            Extend::Pad,
            Affine::translate((0.0, 12.0)) * Affine::scale(4.0),
            Rect::new(0.0, 12.0, 16.0, 16.0),
        );
        let pixels = rig.render(&scene);
        assert_eq!(
            [
                pixels.at(1, 1),
                pixels.at(5, 1),
                pixels.at(1, 5),
                pixels.at(5, 5)
            ],
            [RED, BLUE, BLUE, RED]
        );
        assert_eq!(
            [
                pixels.at(1, 13),
                pixels.at(5, 13),
                pixels.at(9, 13),
                pixels.at(13, 13)
            ],
            [RED, BLUE, BLUE, RED]
        );
        assert_eq!(rig.backend.images.entries.len(), 2);
    }

    /// Past the budget, the bitmaps a render did not draw are released least
    /// recently drawn first, and one drawn again is uploaded again — into the
    /// region the release cleared, without that clear reaching it.
    #[test]
    fn bitmaps_past_the_budget_are_released_and_upload_again_on_return() {
        let mut rig = Rig::new(8, 8);
        let red = bitmap(1, &[RED]);
        let blue = bitmap(1, &[BLUE]);
        // One padded 2×2 upload's bytes.
        rig.backend.images.budget = 16;
        let frame = |image: &ImageData| {
            let mut scene = vello::Scene::new();
            draw(
                &mut scene,
                image,
                ImageQuality::Medium,
                Extend::Pad,
                Affine::scale(8.0),
                Rect::new(0.0, 0.0, 8.0, 8.0),
            );
            scene
        };
        assert_eq!(rig.render(&frame(&red)).at(4, 4), RED);
        assert_eq!(rig.backend.images.bytes, 16);

        // Red stays pinned through the renders after it, then goes; a
        // render drawing no bitmap is not one of them.
        for _ in 1..PINNED_RENDERS {
            assert_eq!(rig.render(&frame(&blue)).at(4, 4), BLUE);
            let _ = rig.render(&vello::Scene::new());
            assert_eq!(rig.backend.images.entries.len(), 2, "red is pinned");
        }
        assert_eq!(rig.render(&frame(&blue)).at(4, 4), BLUE);
        assert_eq!(rig.backend.images.entries.len(), 1, "red was released");
        assert_eq!(rig.backend.images.bytes, 16);
        assert_eq!(rig.backend.images.atlas_bytes, 16);

        for _ in 0..2 {
            for _ in 0..PINNED_RENDERS {
                assert_eq!(rig.render(&frame(&red)).at(4, 4), RED, "uploaded again");
            }
            for _ in 0..PINNED_RENDERS {
                assert_eq!(rig.render(&frame(&blue)).at(4, 4), BLUE);
            }
        }
        assert_eq!(rig.backend.images.entries.len(), 1);

        // A render drawing both keeps both, past the budget, for itself.
        let mut both = frame(&red);
        draw(
            &mut both,
            &blue,
            ImageQuality::Medium,
            Extend::Pad,
            Affine::IDENTITY,
            Rect::new(4.0, 0.0, 8.0, 8.0),
        );
        let pixels = rig.render(&both);
        assert_eq!(pixels.at(1, 4), RED);
        assert_eq!(pixels.at(6, 4), BLUE);
        assert_eq!(rig.backend.images.entries.len(), 2);
        for _ in 0..PINNED_RENDERS {
            let _ = rig.render(&frame(&blue));
        }
        assert_eq!(
            rig.backend.images.entries.len(),
            1,
            "and once it is unpinned, red goes",
        );
    }

    /// A bitmap longer than the atlas takes for bitmaps is drawn from a
    /// texture of its own, and released by dropping it.
    #[test]
    fn a_long_bitmap_draws_from_its_own_texture() {
        let mut rig = Rig::new(16, 4);
        let mut texels = vec![RED; 1100];
        texels[1099] = BLUE;
        let image = bitmap(1100, &texels);
        let mut scene = vello::Scene::new();
        // The last eight texels at pixels 0..8, then the padded area.
        draw(
            &mut scene,
            &image,
            ImageQuality::Low,
            Extend::Pad,
            Affine::translate((-1092.0, 0.0)),
            Rect::new(0.0, 0.0, 16.0, 4.0),
        );
        let pixels = rig.render(&scene);
        assert_eq!(pixels.at(0, 0), RED);
        assert_eq!(pixels.at(7, 0), BLUE);
        assert_eq!(pixels.at(12, 0), BLUE, "padded past the end");
        assert!(matches!(
            rig.backend
                .images
                .entries
                .values()
                .next()
                .map(|bitmap| &bitmap.store),
            Some(Store::Texture { .. })
        ));
        assert_eq!(rig.backend.images.atlas_bytes, 0);
        rig.backend.images.budget = 0;
        let other = bitmap(1, &[RED]);
        for _ in 0..PINNED_RENDERS {
            let mut scene = vello::Scene::new();
            draw(
                &mut scene,
                &other,
                ImageQuality::Low,
                Extend::Pad,
                Affine::IDENTITY,
                Rect::new(0.0, 0.0, 1.0, 1.0),
            );
            let _ = rig.render(&scene);
        }
        assert_eq!(rig.backend.images.entries.len(), 1, "the long one went");
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod parity_tests {
    use vello::kurbo::{Cap, Join, Line, Stroke};
    use vello::peniko::color::palette::css::{BLACK, BLUE, RED};

    use super::*;
    use crate::render::gpu::Headless;

    fn render(scene: &vello::Scene, width: u32, height: u32) -> Vec<u8> {
        let mut gpu = Headless::new().expect("a usable GPU adapter is mandatory");
        gpu.render(scene, &[], width, height, Color::WHITE)
            .expect("the scene renders")
    }

    fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * width + x) * 4) as usize;
        [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
    }

    const WHITE: [u8; 4] = [255, 255, 255, 255];

    fn hard_stop(at: f32) -> [peniko::ColorStop; 4] {
        [
            (0.0, RED).into(),
            (at, RED).into(),
            (at, BLUE).into(),
            (1.0, BLUE).into(),
        ]
    }

    fn vertices(path: &BezPath) -> Vec<(f64, f64)> {
        path.elements()
            .iter()
            .filter_map(|el| match el {
                vello::kurbo::PathEl::MoveTo(p) | vello::kurbo::PathEl::LineTo(p) => {
                    Some((p.x, p.y))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_filled_rect_flattens_to_its_four_device_corners() {
        let mut scene = vello::Scene::new();
        let transform = Affine::translate((3.0, 4.0)) * Affine::scale(2.0);
        scene.fill(
            Fill::NonZero,
            transform,
            RED,
            None,
            &Rect::new(0.0, 0.0, 5.0, 5.0),
        );
        let paths = decode_paths(scene.encoding());
        assert_eq!(paths.len(), 1);
        assert_eq!(
            vertices(&paths[0].path),
            [
                (3.0, 4.0),
                (13.0, 4.0),
                (13.0, 14.0),
                (3.0, 14.0),
                (3.0, 4.0)
            ],
            "each degree-raised line flattens to itself, in device space"
        );
        assert_eq!(paths[0].fill(), Fill::NonZero);
    }

    #[test]
    fn an_open_strokes_cap_marker_draws_its_caps_and_no_closing_side() {
        for (cap, area, x0) in [(Cap::Butt, 20.0, 0.0), (Cap::Square, 24.0, -1.0)] {
            let mut scene = vello::Scene::new();
            let stroke = Stroke::new(2.0).with_caps(cap);
            let line = Line::new((0.0, 5.0), (10.0, 5.0));
            scene.stroke(&stroke, Affine::IDENTITY, BLACK, None, &line);
            let paths = decode_paths(scene.encoding());
            assert_eq!(paths.len(), 1);
            let path = &paths[0].path;
            assert!(
                (path.area().abs() - area).abs() < 1e-3,
                "{cap:?}: {}",
                path.area()
            );
            let bounds = path.bounding_box();
            assert!((bounds.x0 - x0).abs() < 1e-4 && (bounds.y0 - 4.0).abs() < 1e-4);
            assert!((bounds.x1 - (10.0 - x0)).abs() < 1e-4 && (bounds.y1 - 6.0).abs() < 1e-4);
            assert_eq!(
                paths[0].fill(),
                Fill::NonZero,
                "a stroke's lines fill non-zero"
            );
        }
    }

    #[test]
    fn a_closed_strokes_cap_marker_joins_back_to_its_start() {
        let mut scene = vello::Scene::new();
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let stroke = Stroke::new(2.0).with_join(Join::Miter);
        scene.stroke(&stroke, Affine::translate((1.0, 1.0)), BLACK, None, &rect);
        let path = &decode_paths(scene.encoding())[0].path;
        assert_eq!(path.bounding_box(), Rect::new(0.0, 0.0, 12.0, 12.0));
        // Mitred corners, and the 8 px hole left unpainted.
        let pixels = render(&scene, 12, 12);
        let black = [0, 0, 0, 255];
        for (x, y) in [(0, 0), (11, 0), (0, 11), (11, 11), (5, 1)] {
            assert_eq!(pixel(&pixels, 12, x, y), black, "({x}, {y})");
        }
        for (x, y) in [(2, 2), (5, 5), (9, 9)] {
            assert_eq!(pixel(&pixels, 12, x, y), WHITE, "({x}, {y})");
        }
    }

    #[test]
    fn a_brush_transform_is_the_one_current_at_the_path_marker() {
        let mut scene = vello::Scene::new();
        let transform = Affine::translate((5.0, 0.0));
        let brush = Affine::scale(2.0);
        let gradient = Gradient::new_linear((0.0, 0.0), (4.0, 0.0)).with_stops(hard_stop(0.5));
        scene.fill(
            Fill::EvenOdd,
            transform,
            &gradient,
            Some(brush),
            &Rect::new(0.0, 0.0, 4.0, 4.0),
        );
        let paths = decode_paths(scene.encoding());
        assert_eq!(paths[0].brush_transform, transform * brush);
        assert_eq!(paths[0].path.bounding_box(), Rect::new(5.0, 0.0, 9.0, 4.0));
        assert_eq!(paths[0].fill(), Fill::EvenOdd);
    }

    #[test]
    fn the_ramp_is_vellos_make_ramp() {
        let ramp = vello_ramp(
            &hard_stop(0.5),
            peniko::InterpolationAlphaSpace::Premultiplied,
        );
        let red = RED.premultiply().to_rgba8();
        let blue = BLUE.premultiply().to_rgba8();
        assert_eq!(ramp.len(), RAMP_SAMPLES);
        // Sample 255 sits at 0.499, sample 256 at 0.501.
        assert_eq!((ramp[0], ramp[255]), (red, red));
        assert_eq!((ramp[256], ramp[511]), (blue, blue));
    }

    #[test]
    fn a_stair_colour_premultiplies_back_to_its_table_entry() {
        for (r, g, b, a) in [
            (0, 0, 0, 0),
            (12, 200, 7, 200),
            (1, 1, 1, 1),
            (255, 255, 255, 255),
        ] {
            let rgba = PremulRgba8 { r, g, b, a };
            let [cr, cg, cb, ca] = stair_color(rgba).premultiply().components;
            let byte = |c: f32| (c * 255.0 + 0.5) as u8;
            assert_eq!([byte(cr), byte(cg), byte(cb), byte(ca)], [r, g, b, a]);
        }
    }

    /// vello's sample at `t`, a tie at a sample boundary sent up.
    fn vello_index(t: f64, extend: Extend) -> usize {
        let x = match extend {
            Extend::Pad => t.clamp(0.0, 1.0),
            _ => t - t.floor(),
        };
        ((x * 511.0 + 0.5).floor() as usize).min(511)
    }

    #[test]
    fn under_pad_each_table_entry_reads_the_sample_vello_rounds_to() {
        let map = RampMap::new(Extend::Pad);
        for i in 0..200_000 {
            let t = f64::from(i) / 200_000.0 * 1.2 - 0.1;
            let u = (map.scale * t + map.bias).clamp(0.0, 1.0);
            let k = ((u * 4095.0).floor() as usize).min(LUT_WIDTH - 1);
            let (lo, hi) = map.samples(k);
            // Within the tie bias below a boundary the entry reads upwards.
            let near_tie = (t * 511.0 + 0.5).fract() > 1.0 - TIE_BIAS / 8.0;
            if !near_tie {
                assert_eq!(hi, vello_index(t, Extend::Pad), "t = {t}");
            }
            assert!(lo <= hi);
        }
    }

    #[test]
    fn under_repeat_each_table_entry_reads_the_sample_vello_rounds_to() {
        let map = RampMap::new(Extend::Repeat);
        for i in 0..200_000 {
            let t = f64::from(i) / 200_000.0 * 3.0 - 1.0;
            let u = map.scale * t + map.bias;
            let k = (((u - u.floor()) * 4095.0).floor() as usize).min(LUT_WIDTH - 1);
            let (lo, hi) = map.samples(k);
            let expected = vello_index(t, Extend::Repeat);
            // An entry a sample boundary crosses reads one side of it.
            assert!(
                hi == expected || lo == expected,
                "t = {t}: {lo} {hi} {expected}"
            );
        }
    }

    #[test]
    fn a_tie_follows_vellos_fused_parameter() {
        // A 54 px mask's 50 % stop on a pixel edge, at two device offsets
        // (css_atlas `mask-position-005` and `-002`): vello reads the upper
        // sample at one and the lower at the other.
        for (x0, upper) in [(64.0_f32, true), (320.0, false)] {
            let transform = Transform {
                matrix: [1.0, 0.0, 0.0, 1.0],
                translation: [x0, 0.0],
            };
            let line = VelloLine::new(&transform, [0.0, 21.0], [54.0, 21.0]);
            let sample = vello_sample(line.at(x0 as i32 + 27, 0), Extend::Pad);
            assert_eq!(sample, if upper { 256 } else { 255 }, "at {x0}");
        }
    }

    #[test]
    fn a_hard_stop_lands_on_the_column_vello_samples_at_its_corner() {
        let mut scene = vello::Scene::new();
        let gradient = Gradient::new_linear((0.0, 0.0), (20.0, 0.0)).with_stops(hard_stop(0.475));
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            &gradient,
            None,
            &Rect::new(0.0, 0.0, 20.0, 2.0),
        );
        let pixels = render(&scene, 20, 2);
        // The stop is at x = 9.5: column 9's corner is before it, its centre
        // after it.
        assert_eq!(pixel(&pixels, 20, 9, 0), [255, 0, 0, 255]);
        assert_eq!(pixel(&pixels, 20, 10, 0), [0, 0, 255, 255]);
    }

    #[test]
    fn a_conic_centre_on_a_pixel_corner_is_left_unpainted() {
        let mut scene = vello::Scene::new();
        let gradient = Gradient::new_sweep((8.0, 8.0), 0.0, std::f32::consts::TAU)
            .with_stops([RED, BLUE, RED].as_slice())
            .with_extend(Extend::Repeat);
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            &gradient,
            None,
            &Rect::new(0.0, 0.0, 16.0, 16.0),
        );
        let pixels = render(&scene, 16, 16);
        assert_eq!(pixel(&pixels, 16, 8, 8), WHITE);
        for (x, y) in [(7, 7), (8, 7), (7, 8), (9, 9)] {
            assert_ne!(pixel(&pixels, 16, x, y), WHITE, "({x}, {y})");
        }
    }

    #[test]
    fn a_clip_layer_bounds_the_fill_inside_it() {
        let mut scene = vello::Scene::new();
        scene.push_clip_layer(
            Fill::NonZero,
            Affine::IDENTITY,
            &Rect::new(2.0, 2.0, 6.0, 6.0),
        );
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            RED,
            None,
            &Rect::new(0.0, 0.0, 8.0, 8.0),
        );
        scene.pop_layer();
        let pixels = render(&scene, 8, 8);
        assert_eq!(pixel(&pixels, 8, 1, 1), WHITE);
        assert_eq!(pixel(&pixels, 8, 3, 3), [255, 0, 0, 255]);
        assert_eq!(pixel(&pixels, 8, 6, 6), WHITE);
    }

    #[test]
    fn a_stroke_covers_its_offsets_and_its_caps() {
        for (cap, capped) in [(Cap::Butt, false), (Cap::Square, true)] {
            let mut scene = vello::Scene::new();
            let line = Line::new((2.0, 4.0), (14.0, 4.0));
            let stroke = Stroke::new(2.0).with_caps(cap);
            scene.stroke(&stroke, Affine::IDENTITY, BLACK, None, &line);
            let pixels = render(&scene, 16, 8);
            let black = [0, 0, 0, 255];
            assert_eq!(pixel(&pixels, 16, 5, 3), black);
            assert_eq!(pixel(&pixels, 16, 5, 4), black);
            assert_eq!(pixel(&pixels, 16, 5, 2), WHITE);
            assert_eq!(pixel(&pixels, 16, 1, 3) == black, capped, "{cap:?}");
        }
    }
}
