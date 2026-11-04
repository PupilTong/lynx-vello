//! vello's flattening, on the CPU: a path's segments turned into the lines
//! vello's `flatten` stage writes for them, in device space.
//!
//! `vello_gpu` flattens a curve to its own polyline, within the same 0.25 px
//! tolerance as vello but through other subdivisions, so an edge pixel on a
//! curve can differ by a quarter of its coverage. Handing `vello_gpu` the
//! lines vello itself would rasterize leaves nothing for it to flatten. This
//! is `vello_shaders`' `flatten.wgsl` (0.11) function for function, in `f32`:
//! every segment degree-raised to a cubic, split into Euler-spiral pieces and
//! each piece's parallel curve flattened; a stroke draws both offsets of
//! every segment, its joins and its caps.

#![allow(
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::suboptimal_flops,
    reason = "a transliteration of vello's shader: its names and its operation order"
)]

use rustc_hash::FxHashMap;
use vello::kurbo::{BezPath, Point};
use vello::low_level::vello_encoding::{PathTag, Style, Transform};

type V = [f32; 2];

fn add(a: V, b: V) -> V {
    [a[0] + b[0], a[1] + b[1]]
}

fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1]]
}

fn mul(a: V, s: f32) -> V {
    [a[0] * s, a[1] * s]
}

fn dot(a: V, b: V) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn length(a: V) -> f32 {
    dot(a, a).sqrt()
}

fn normalize(a: V) -> V {
    mul(a, 1.0 / length(a))
}

/// `n.yx * vec2(-1, 1)`.
fn perp(a: V) -> V {
    [-a[1], a[0]]
}

fn apply(t: &Transform, p: V) -> V {
    let [a, b, c, d] = t.matrix;
    let [e, f] = t.translation;
    [
        a.mul_add(p[0], c.mul_add(p[1], e)),
        b.mul_add(p[0], d.mul_add(p[1], f)),
    ]
}

const IDENTITY: Transform = Transform::IDENTITY;

/// A path tag's segment type bits, and the two types a cubic is raised from.
const SEG_TYPE: u8 = 0x3;
const LINE_TO: u8 = 0x1;
const QUAD_TO: u8 = 0x2;

/// One segment tag of a path and the points it reads, as the shader's
/// `read_path_segment` sees them before degree raising: `points[0]` is the
/// previous segment's end.
#[derive(Clone, Copy, Debug)]
pub(super) struct Segment {
    pub(super) tag: u8,
    pub(super) points: [V; 4],
}

/// The segment's cubic, as `read_path_segment` builds it.
fn cubic(segment: &Segment, is_stroke: bool) -> [V; 4] {
    let [mut p0, mut p1, mut p2, mut p3] = segment.points;
    let mut seg_type = segment.tag & SEG_TYPE;
    let is_cap_marker = is_stroke && segment.tag & PathTag::SUBPATH_END_BIT != 0;
    if is_cap_marker && seg_type == QUAD_TO {
        p0 = p1;
        p1 = p2;
        seg_type = LINE_TO;
    }
    if seg_type == LINE_TO {
        p3 = p1;
        p2 = add(p3, mul(sub(p0, p3), 1.0 / 3.0));
        p1 = add(p0, mul(sub(p3, p0), 1.0 / 3.0));
    } else if seg_type == QUAD_TO {
        p3 = p2;
        p2 = add(p1, mul(sub(p2, p1), 1.0 / 3.0));
        p1 = add(p1, mul(sub(p0, p1), 1.0 / 3.0));
    }
    [p0, p1, p2, p3]
}

/// Where lines go: in order for a fill, whose lines chain end to start; as a
/// soup for a stroke, whose offset curves, joins and caps vello writes in no
/// connected order.
struct Lines {
    lines: Vec<(V, V)>,
}

impl Lines {
    fn line(&mut self, p0: V, p1: V) {
        self.lines.push((p0, p1));
    }

    fn line_with(&mut self, p0: V, p1: V, t: &Transform) {
        self.lines.push((apply(t, p0), apply(t, p1)));
    }
}

const DERIV_THRESH: f32 = 1e-6;
const DERIV_THRESH_SQUARED: f32 = DERIV_THRESH * DERIV_THRESH;
const DERIV_EPS: f32 = 1e-6;
const SUBDIV_LIMIT: f32 = 1.0 / 65536.0;
const K1_THRESH: f32 = 1e-3;
const DIST_THRESH: f32 = 1e-3;
const TANGENT_THRESH: f32 = 1e-6;
const TOL: f32 = 0.25;

struct CubicParams {
    th0: f32,
    th1: f32,
    chord_len: f32,
    err: f32,
}

struct EulerParams {
    th0: f32,
    k0: f32,
    k1: f32,
    ch: f32,
}

fn cubic_from_points_derivs(p0: V, p1: V, q0: V, q1: V, dt: f32) -> CubicParams {
    let chord = sub(p1, p0);
    let chord_squared = dot(chord, chord);
    let chord_len = chord_squared.sqrt();
    if chord_squared < DERIV_THRESH_SQUARED {
        let chord_err = ((9. / 32.0) * (dot(q0, q0) + dot(q1, q1))).sqrt() * dt;
        return CubicParams {
            th0: 0.0,
            th1: 0.0,
            chord_len: DERIV_THRESH,
            err: chord_err,
        };
    }
    let scale = dt / chord_squared;
    let h0 = [
        q0[0] * chord[0] + q0[1] * chord[1],
        q0[1] * chord[0] - q0[0] * chord[1],
    ];
    let th0 = h0[1].atan2(h0[0]);
    let d0 = length(h0) * scale;
    let h1 = [
        q1[0] * chord[0] + q1[1] * chord[1],
        q1[0] * chord[1] - q1[1] * chord[0],
    ];
    let th1 = h1[1].atan2(h1[0]);
    let d1 = length(h1) * scale;
    let cth0 = th0.cos();
    let cth1 = th1.cos();
    let mut err = 2.0;
    if cth0 * cth1 >= 0.0 {
        let e0 = (2. / 3.) / (1.0 + cth0).max(1e-9);
        let e1 = (2. / 3.) / (1.0 + cth1).max(1e-9);
        let s0 = th0.sin();
        let s1 = th1.sin();
        let s01 = cth0 * s1 + cth1 * s0;
        let amin = 0.15 * (2. * e0 * s0 + 2. * e1 * s1 - e0 * e1 * s01);
        let a = 0.15 * (2. * d0 * s0 + 2. * d1 * s1 - d0 * d1 * s01);
        let aerr = (a - amin).abs();
        let symm = (th0 + th1).abs();
        let asymm = (th0 - th1).abs();
        let dist = length([d0 - e0, d1 - e1]);
        let symm2 = symm * symm;
        let ctr = (4.625e-6 * symm * symm2 + 7.5e-3 * asymm) * symm2;
        let halo = (5e-3 * symm + 7e-2 * asymm) * dist;
        err = ctr + 1.55 * aerr + halo;
    }
    err *= chord_len;
    CubicParams {
        th0,
        th1,
        chord_len,
        err,
    }
}

fn es_params_from_angles(th0: f32, th1: f32) -> EulerParams {
    let k0 = th0 + th1;
    let dth = th1 - th0;
    let d2 = dth * dth;
    let k2 = k0 * k0;
    let mut a = 6.0;
    a -= d2 * (1. / 70.);
    a -= (d2 * d2) * (1. / 10780.);
    a += (d2 * d2 * d2) * 2.769_178_2e-7;
    let b = -0.1 + d2 * (1. / 4200.) + d2 * d2 * 1.695_967_8e-5;
    let c = -1. / 1400. + d2 * 6.849_16e-5 - k2 * 7.936_475e-6;
    a += (b + c * k2) * k2;
    let k1 = dth * a;
    let mut ch = 1.0;
    ch -= d2 * (1. / 40.);
    ch += (d2 * d2) * 0.000_342_261_9;
    ch -= (d2 * d2 * d2) * 1.934_947_5e-6;
    let b_ = -1. / 24. + d2 * 0.002_470_238 - d2 * d2 * 3.729_741e-5;
    let c_ = 1. / 1920. - d2 * 4.873_508_7e-5 - k2 * 3.100_193_6e-6;
    ch += (b_ + c_ * k2) * k2;
    EulerParams { th0, k0, k1, ch }
}

fn es_params_eval_th(params: &EulerParams, t: f32) -> f32 {
    (params.k0 + 0.5 * params.k1 * (t - 1.0)) * t - params.th0
}

fn integ_euler_10(k0: f32, k1: f32) -> V {
    let t1_1 = k0;
    let t1_2 = 0.5 * k1;
    let t2_2 = t1_1 * t1_1;
    let t2_3 = 2. * (t1_1 * t1_2);
    let t2_4 = t1_2 * t1_2;
    let t3_4 = t2_2 * t1_2 + t2_3 * t1_1;
    let t3_6 = t2_4 * t1_2;
    let t4_4 = t2_2 * t2_2;
    let t4_5 = 2. * (t2_2 * t2_3);
    let t4_6 = 2. * (t2_2 * t2_4) + t2_3 * t2_3;
    let t4_7 = 2. * (t2_3 * t2_4);
    let t4_8 = t2_4 * t2_4;
    let t5_6 = t4_4 * t1_2 + t4_5 * t1_1;
    let t5_8 = t4_6 * t1_2 + t4_7 * t1_1;
    let t6_6 = t4_4 * t2_2;
    let t6_7 = t4_4 * t2_3 + t4_5 * t2_2;
    let t6_8 = t4_4 * t2_4 + t4_5 * t2_3 + t4_6 * t2_2;
    let t7_8 = t6_6 * t1_2 + t6_7 * t1_1;
    let t8_8 = t6_6 * t2_2;
    let mut u = 1.;
    u -= (1. / 24.) * t2_2 + (1. / 160.) * t2_4;
    u += (1. / 1920.) * t4_4 + (1. / 10752.) * t4_6 + (1. / 55296.) * t4_8;
    u -= (1. / 322_560.) * t6_6 + (1. / 1_658_880.) * t6_8;
    u += (1. / 92_897_280.) * t8_8;
    let mut v = (1. / 12.) * t1_2;
    v -= (1. / 480.) * t3_4 + (1. / 2688.) * t3_6;
    v += (1. / 53760.) * t5_6 + (1. / 276_480.) * t5_8;
    v -= (1. / 11_612_160.) * t7_8;
    [u, v]
}

fn es_params_eval(params: &EulerParams, t: f32) -> V {
    let thm = es_params_eval_th(params, t * 0.5);
    let k0 = params.k0;
    let k1 = params.k1;
    let uv = integ_euler_10((k0 + k1 * (0.5 * t - 0.5)) * t, k1 * t * t);
    let scale = t / params.ch;
    let s = scale * thm.sin();
    let c = scale * thm.cos();
    let x = uv[0] * c - uv[1] * s;
    let y = -uv[1] * c - uv[0] * s;
    [x, y]
}

fn es_params_eval_with_offset(params: &EulerParams, t: f32, offset: f32) -> V {
    let th = es_params_eval_th(params, t);
    let v = mul([th.sin(), th.cos()], offset);
    add(es_params_eval(params, t), v)
}

fn es_seg_eval_with_offset(p0: V, p1: V, params: &EulerParams, t: f32, offset: f32) -> V {
    let chord = sub(p1, p0);
    let xy = es_params_eval_with_offset(params, t, offset);
    add(
        p0,
        [
            chord[0] * xy[0] - chord[1] * xy[1],
            chord[0] * xy[1] + chord[1] * xy[0],
        ],
    )
}

fn pow_1_5_signed(x: f32) -> f32 {
    x * x.abs().sqrt()
}

/// WGSL's `sign`: zero at zero.
fn sign(x: f32) -> f32 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }
}

const BREAK1: f32 = 0.8;
const BREAK2: f32 = 1.25;
const BREAK3: f32 = 2.1;
const SIN_SCALE: f32 = 1.097_699_2;
const QUAD_A1: f32 = 0.6406;
const QUAD_B1: f32 = -0.81;
const QUAD_C1: f32 = 0.914_811_8;
const QUAD_A2: f32 = 0.5;
const QUAD_B2: f32 = -0.156;
const QUAD_C2: f32 = 0.161_457_8;
const QUAD_W1: f32 = 0.5 * QUAD_B1 / QUAD_A1;
const QUAD_V1: f32 = 1.0 / QUAD_A1;
const QUAD_U1: f32 = QUAD_W1 * QUAD_W1 - QUAD_C1 / QUAD_A1;
const QUAD_W2: f32 = 0.5 * QUAD_B2 / QUAD_A2;
const QUAD_V2: f32 = 1.0 / QUAD_A2;
const QUAD_U2: f32 = QUAD_W2 * QUAD_W2 - QUAD_C2 / QUAD_A2;
const FRAC_PI_4: f32 = std::f32::consts::FRAC_PI_4;
const CBRT_9_8: f32 = 1.040_041_9;

fn espc_int_approx(x: f32) -> f32 {
    let y = x.abs();
    let a = if y < BREAK1 {
        (SIN_SCALE * y).sin() * (1.0 / SIN_SCALE)
    } else if y < BREAK2 {
        (8.0_f32.sqrt() / 3.0) * pow_1_5_signed(y - 1.0) + FRAC_PI_4
    } else {
        let (qa, qb, qc) = if y < BREAK3 {
            (QUAD_A1, QUAD_B1, QUAD_C1)
        } else {
            (QUAD_A2, QUAD_B2, QUAD_C2)
        };
        (qa * y + qb) * y + qc
    };
    a * sign(x)
}

fn espc_int_inv_approx(x: f32) -> f32 {
    let y = x.abs();
    let a = if y < 0.701_070_76 {
        (y * SIN_SCALE).asin() * (1.0 / SIN_SCALE)
    } else if y < 0.903_249_3 {
        let b = y - FRAC_PI_4;
        let u = b.abs().powf(2. / 3.) * sign(b);
        u * CBRT_9_8 + 1.0
    } else {
        let (u, v, w) = if y < 2.038_857_8 {
            (QUAD_U1, QUAD_V1, QUAD_W1)
        } else {
            (QUAD_U2, QUAD_V2, QUAD_W2)
        };
        (u + v * y).sqrt() - w
    };
    a * sign(x)
}

fn eval_cubic_and_deriv(p0: V, p1: V, p2: V, p3: V, t: f32) -> (V, V) {
    let m = 1.0 - t;
    let mm = m * m;
    let mt = m * t;
    let tt = t * t;
    let p = add(
        mul(p0, mm * m),
        mul(
            add(add(mul(p1, 3.0 * mm), mul(p2, 3.0 * mt)), mul(p3, tt)),
            t,
        ),
    );
    let q = add(
        add(mul(sub(p1, p0), mm), mul(sub(p2, p1), 2.0 * mt)),
        mul(sub(p3, p2), tt),
    );
    (p, q)
}

fn cubic_start_tangent([p0, p1, p2, p3]: [V; 4]) -> V {
    let eps = 1e-12;
    let d01 = sub(p1, p0);
    let d02 = sub(p2, p0);
    let d03 = sub(p3, p0);
    if dot(d01, d01) > eps {
        d01
    } else if dot(d02, d02) > eps {
        d02
    } else {
        d03
    }
}

fn cubic_end_tangent([p0, p1, p2, p3]: [V; 4]) -> V {
    let eps = 1e-12;
    let d23 = sub(p3, p2);
    let d13 = sub(p3, p1);
    let d03 = sub(p3, p0);
    if dot(d23, d23) > eps {
        d23
    } else if dot(d13, d13) > eps {
        d13
    } else {
        d03
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Robust {
    Normal,
    LowK1,
    LowDist,
}

fn flatten_euler(
    out: &mut Lines,
    cubic: [V; 4],
    local_to_device: &Transform,
    offset: f32,
    start_p: V,
    end_p: V,
) {
    let (p0, p1, p2, p3, scale, transform, t_start, t_end);
    if offset == 0. {
        let t = local_to_device;
        p0 = apply(t, cubic[0]);
        p1 = apply(t, cubic[1]);
        p2 = apply(t, cubic[2]);
        p3 = apply(t, cubic[3]);
        scale = 1.;
        transform = IDENTITY;
        t_start = p0;
        t_end = p3;
    } else {
        [p0, p1, p2, p3] = cubic;
        transform = *local_to_device;
        let [mx, my, mz, mw] = transform.matrix;
        scale = 0.5 * (length([mx + mw, my - mz]) + length([mx - mw, my + mz]));
        t_start = start_p;
        t_end = end_p;
    }
    if p0 == p1 && p0 == p2 && p0 == p3 {
        return;
    }
    let mut t0_u = 0_u32;
    let mut dt = 1.0_f32;
    let mut last_p = p0;
    let mut last_q = sub(p1, p0);
    if dot(last_q, last_q) < DERIV_THRESH_SQUARED {
        last_q = eval_cubic_and_deriv(p0, p1, p2, p3, DERIV_EPS).1;
    }
    let mut last_t = 0.0_f32;
    let mut lp0 = t_start;
    loop {
        let t0 = t0_u as f32 * dt;
        if t0 == 1.0 {
            break;
        }
        let mut t1 = t0 + dt;
        let this_p0 = last_p;
        let this_q0 = last_q;
        let (mut point, mut deriv) = eval_cubic_and_deriv(p0, p1, p2, p3, t1);
        if dot(deriv, deriv) < DERIV_THRESH_SQUARED {
            let (new_point, new_deriv) = eval_cubic_and_deriv(p0, p1, p2, p3, t1 - DERIV_EPS);
            deriv = new_deriv;
            if t1 < 1.0 {
                point = new_point;
                t1 -= DERIV_EPS;
            }
        }
        let actual_dt = t1 - last_t;
        let cp = cubic_from_points_derivs(this_p0, point, this_q0, deriv, actual_dt);
        if cp.err * scale <= TOL || dt <= SUBDIV_LIMIT {
            let params = es_params_from_angles(cp.th0, cp.th1);
            let k0 = params.k0 - 0.5 * params.k1;
            let k1 = params.k1;
            let normalized_offset = offset / cp.chord_len;
            let dist_scaled = normalized_offset * params.ch;
            let scale_multiplier = (0.125 * scale * cp.chord_len / (params.ch * TOL)).sqrt();
            let (mut a, mut b, mut integral, mut int0) = (0.0, 0.0, 0.0, 0.0);
            let n_frac;
            let robust;
            if k1.abs() < K1_THRESH {
                let k = params.k0;
                n_frac = (k * (k * dist_scaled + 1.0)).abs().sqrt();
                robust = Robust::LowK1;
            } else if dist_scaled.abs() < DIST_THRESH {
                a = k1;
                b = k0;
                int0 = pow_1_5_signed(b);
                let int1 = pow_1_5_signed(a + b);
                integral = int1 - int0;
                n_frac = (2. / 3.) * integral / a;
                robust = Robust::LowDist;
            } else {
                a = -2.0 * dist_scaled * k1;
                b = -1.0 - 2.0 * dist_scaled * k0;
                int0 = espc_int_approx(b);
                let int1 = espc_int_approx(a + b);
                integral = int1 - int0;
                let k_peak = k0 - k1 * b / a;
                let integrand_peak = (k_peak * (k_peak * dist_scaled + 1.0)).abs().sqrt();
                n_frac = integral * integrand_peak / a;
                robust = Robust::Normal;
            }
            let n = (n_frac * scale_multiplier).ceil().clamp(1.0, 100.0);
            // `u32(NaN)` is zero in WGSL, as `as u32` is here.
            let count = n as u32;
            for i in 0..count {
                let lp1 = if i + 1 == count && t1 == 1.0 {
                    t_end
                } else {
                    let t = (i + 1) as f32 / n;
                    let mut s = t;
                    if robust != Robust::LowK1 {
                        let u = integral * t + int0;
                        let inv = if robust == Robust::LowDist {
                            u.abs().powf(2. / 3.) * sign(u)
                        } else {
                            espc_int_inv_approx(u)
                        };
                        s = (inv - b) / a;
                    }
                    es_seg_eval_with_offset(this_p0, point, &params, s, normalized_offset)
                };
                let (l0, l1) = if offset >= 0. { (lp0, lp1) } else { (lp1, lp0) };
                out.line_with(l0, l1, &transform);
                lp0 = lp1;
            }
            last_p = point;
            last_q = deriv;
            last_t = t1;
            t0_u += 1;
            let shift = t0_u.trailing_zeros();
            t0_u >>= shift;
            dt *= (1_u32 << shift) as f32;
        } else {
            t0_u *= 2;
            dt *= 0.5;
        }
    }
}

fn flatten_arc(out: &mut Lines, begin: V, end: V, center: V, angle: f32, transform: &Transform) {
    let mut p0 = apply(transform, begin);
    let mut r = sub(begin, center);
    let min_theta: f32 = 0.0001;
    let radius = TOL.max(length(sub(p0, apply(transform, center))));
    let theta = min_theta.max(2. * (1. - TOL / radius).acos());
    let n_lines = 1_u32.max((angle / theta).ceil() as u32);
    let c = theta.cos();
    let s = theta.sin();
    for _ in 0..n_lines - 1 {
        // `mat2x2(c, -s, s, c) * r`: columns (c, -s) and (s, c).
        r = [c * r[0] + s * r[1], -s * r[0] + c * r[1]];
        let p1 = apply(transform, add(center, r));
        out.line(p0, p1);
        p0 = p1;
    }
    let p1 = apply(transform, end);
    out.line(p0, p1);
}

fn draw_cap(
    out: &mut Lines,
    cap_style: u32,
    point: V,
    cap0: V,
    cap1: V,
    offset_tangent: V,
    transform: &Transform,
) {
    if cap_style == Style::FLAGS_CAP_BITS_ROUND {
        flatten_arc(out, cap0, cap1, point, 3.141_592_7, transform);
        return;
    }
    let (start, end) = (cap0, cap1);
    if cap_style == Style::FLAGS_CAP_BITS_SQUARE {
        let p0 = add(start, offset_tangent);
        let p1 = add(end, offset_tangent);
        out.line_with(start, p0, transform);
        out.line_with(p1, end, transform);
        out.line_with(p0, p1, transform);
        return;
    }
    out.line_with(start, end, transform);
}

#[expect(clippy::too_many_arguments, reason = "the shader's signature")]
fn draw_join(
    out: &mut Lines,
    style_flags: u32,
    p0: V,
    tan_prev: V,
    tan_next: V,
    n_prev: V,
    n_next: V,
    transform: &Transform,
) {
    let mut front0 = add(p0, n_prev);
    let front1 = add(p0, n_next);
    let mut back0 = sub(p0, n_next);
    let back1 = sub(p0, n_prev);
    let cr = tan_prev[0] * tan_next[1] - tan_prev[1] * tan_next[0];
    let d = dot(tan_prev, tan_next);
    match style_flags & Style::FLAGS_JOIN_MASK {
        Style::FLAGS_JOIN_BITS_BEVEL => {
            out.line_with(front0, front1, transform);
            out.line_with(back0, back1, transform);
        }
        Style::FLAGS_JOIN_BITS_MITER => {
            let hypot = length([cr, d]);
            let miter_limit = f16_to_f32((style_flags & Style::MITER_LIMIT_MASK) as u16);
            if 2. * hypot < (hypot + d) * miter_limit * miter_limit
                && cr.abs() > TANGENT_THRESH * TANGENT_THRESH
            {
                let is_backside = cr > 0.;
                let fp_last = if is_backside { back1 } else { front0 };
                let fp_this = if is_backside { back0 } else { front1 };
                let p = if is_backside { back0 } else { front0 };
                let v = sub(fp_this, fp_last);
                let h = (tan_prev[0] * v[1] - tan_prev[1] * v[0]) / cr;
                let miter_pt = sub(fp_this, mul(tan_next, h));
                out.line_with(p, miter_pt, transform);
                if is_backside {
                    back0 = miter_pt;
                } else {
                    front0 = miter_pt;
                }
            }
            out.line_with(front0, front1, transform);
            out.line_with(back0, back1, transform);
        }
        Style::FLAGS_JOIN_BITS_ROUND => {
            let (arc0, arc1, other0, other1) = if cr > 0. {
                (back0, back1, front0, front1)
            } else {
                (front0, front1, back0, back1)
            };
            flatten_arc(out, arc0, arc1, p0, cr.atan2(d).abs(), transform);
            out.line_with(other0, other1, transform);
        }
        _ => {}
    }
}

/// WGSL's `unpack2x16float(x)[0]`.
fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    match exponent {
        0 => sign * mantissa * 2f32.powi(-24),
        0x1f => sign * f32::INFINITY,
        _ => sign * (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15),
    }
}

/// The lines vello's `flatten` writes for one path's `segments` under
/// `style` and `transform`, as a device-space path a fill rasterizes the same
/// way: each closed chain of lines one subpath.
pub(super) fn flatten(segments: &[Segment], style: &Style, transform: &Transform) -> BezPath {
    let flags = style.flags_and_miter_limit;
    let is_stroke = flags & Style::FLAGS_STYLE_BIT != 0;
    let mut out = Lines { lines: Vec::new() };
    let mut path = BezPath::new();
    let point = |v: V| Point::new(f64::from(v[0]), f64::from(v[1]));
    if !is_stroke {
        let mut open = false;
        for segment in segments {
            let pts = cubic(segment, false);
            let first = out.lines.len();
            flatten_euler(&mut out, pts, transform, 0.0, pts[0], pts[3]);
            for &(p0, p1) in &out.lines[first..] {
                if !open {
                    path.move_to(point(p0));
                    open = true;
                }
                path.line_to(point(p1));
            }
            out.lines.truncate(first);
            if segment.tag & PathTag::SUBPATH_END_BIT != 0 && open {
                path.close_path();
                open = false;
            }
        }
        if open {
            path.close_path();
        }
        return path;
    }
    let offset = 0.5 * style.line_width;
    for (ix, segment) in segments.iter().enumerate() {
        let pts = cubic(segment, true);
        let seg_type = segment.tag & SEG_TYPE;
        let is_open = seg_type != LINE_TO;
        let is_cap_marker = segment.tag & PathTag::SUBPATH_END_BIT != 0;
        if is_cap_marker {
            if is_open {
                let tangent = sub(pts[3], pts[0]);
                let offset_tangent = mul(normalize(tangent), offset);
                let n = perp(offset_tangent);
                draw_cap(
                    &mut out,
                    (flags & Style::FLAGS_START_CAP_MASK) >> 2,
                    pts[0],
                    sub(pts[0], n),
                    add(pts[0], n),
                    mul(offset_tangent, -1.0),
                    transform,
                );
            }
            continue;
        }
        let Some(next) = segments.get(ix + 1) else {
            continue;
        };
        let next_pts = cubic(next, true);
        let next_closed = next.tag & SEG_TYPE == LINE_TO;
        let next_marker = next.tag & PathTag::SUBPATH_END_BIT != 0;
        let do_join = !next_marker || next_closed;
        let mut tan_next = if next_marker {
            sub(next_pts[3], next_pts[0])
        } else {
            cubic_start_tangent(next_pts)
        };
        let threshold = TANGENT_THRESH * TANGENT_THRESH;
        let mut tan_start = cubic_start_tangent(pts);
        if dot(tan_start, tan_start) < threshold {
            tan_start = [TANGENT_THRESH, 0.];
        }
        let mut tan_prev = cubic_end_tangent(pts);
        if dot(tan_prev, tan_prev) < threshold {
            tan_prev = [TANGENT_THRESH, 0.];
        }
        if dot(tan_next, tan_next) < threshold {
            tan_next = [TANGENT_THRESH, 0.];
        }
        let n_start = mul(normalize(perp(tan_start)), offset);
        let offset_tangent = mul(normalize(tan_prev), offset);
        let n_prev = perp(offset_tangent);
        let n_next = mul(perp(normalize(tan_next)), offset);
        flatten_euler(
            &mut out,
            pts,
            transform,
            offset,
            add(pts[0], n_start),
            add(pts[3], n_prev),
        );
        flatten_euler(
            &mut out,
            pts,
            transform,
            -offset,
            sub(pts[0], n_start),
            sub(pts[3], n_prev),
        );
        if do_join {
            draw_join(
                &mut out, flags, pts[3], tan_prev, tan_next, n_prev, n_next, transform,
            );
        } else {
            draw_cap(
                &mut out,
                flags & Style::FLAGS_END_CAP_MASK,
                pts[3],
                add(pts[3], n_prev),
                sub(pts[3], n_prev),
                offset_tangent,
                transform,
            );
        }
    }
    chain(&out.lines, &mut path);
    path
}

/// `lines` as closed subpaths: each line follows one starting where it
/// ends. vello's stroke lines close up exactly, its points computed once
/// and shared; a chain that does not close is closed by a chord.
fn chain(lines: &[(V, V)], path: &mut BezPath) {
    let key = |v: V| (v[0].to_bits(), v[1].to_bits());
    let mut starting: FxHashMap<(u32, u32), Vec<usize>> = FxHashMap::default();
    for (index, (p0, _)) in lines.iter().enumerate() {
        starting.entry(key(*p0)).or_default().push(index);
    }
    let mut used = vec![false; lines.len()];
    let point = |v: V| Point::new(f64::from(v[0]), f64::from(v[1]));
    for first in 0..lines.len() {
        if used[first] {
            continue;
        }
        used[first] = true;
        let (start, mut end) = lines[first];
        path.move_to(point(start));
        path.line_to(point(end));
        while key(end) != key(start) {
            let Some(next) = starting.get_mut(&key(end)).and_then(|candidates| {
                while let Some(&candidate) = candidates.last() {
                    if used[candidate] {
                        candidates.pop();
                    } else {
                        return Some(candidate);
                    }
                }
                None
            }) else {
                break;
            };
            used[next] = true;
            end = lines[next].1;
            path.line_to(point(end));
        }
        path.close_path();
    }
}
