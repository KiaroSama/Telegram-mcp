//! Qt 5.15 CPU reference XYZ matrix + Bradford D50 + TRC conversion to sRGB.

use super::{IccOutcome, PixelLayout, be16, be32, bei32, qgray};
use std::{collections::BTreeMap, sync::OnceLock};

#[rustfmt::skip]
const SRGB_PARAMS: [f32; 7] = [1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045, 0.0, 0.0, 2.4];

#[rustfmt::skip]
const PRIMARIES: [[(f32, f32); 4]; 4] = [
    [(0.64, 0.33), (0.3, 0.6), (0.15, 0.06), (0.31271, 0.32902)],
    [(0.64, 0.33), (0.21, 0.71), (0.15, 0.06), (0.31271, 0.32902)],
    [(0.68, 0.32), (0.265, 0.69), (0.15, 0.06), (0.31271, 0.32902)],
    [(0.7347, 0.2653), (0.1596, 0.8404), (0.0366, 0.0001), (0.34567, 0.35850)],
];

#[rustfmt::skip]
const RECOGNIZED: [[[f32; 3]; 3]; 4] = [
    [[0.436_021_75, 0.222_475_11, 0.013_928_114], [0.385_108_8, 0.716_906_7, 0.097_101_54], [0.143_081_25, 0.060_618_2, 0.714_158_6]],
    [[0.609_718_9, 0.311_102_15, 0.019_476_634], [0.205_268_22, 0.625_677_05, 0.060_889_15], [0.149_224_74, 0.063_220_96, 0.744_822_44]],
    [[0.515_097_3, 0.241_179_56, -0.001_049_103_4], [0.291_969_66, 0.692_244_1, 0.041_883_033], [0.157_144_92, 0.066_576_45, 0.784_354_27]],
    [[0.797_667_26, 0.288_037_45, 0.0], [0.135_192_25, 0.711_877, 0.0], [0.031_352_53, 0.000_085_662_7, 0.825_188_34]],
];

#[derive(Clone)]
enum QtCurve {
    Func([f32; 7]),
    Table(Vec<f32>),
}

pub(super) fn valid_pcs_illuminant(pcs_xyz: &[u8]) -> bool {
    tol3(
        s15_xyz(pcs_xyz),
        [0.34567 / 0.35850, 1.0, (1.0 - 0.34567 - 0.35850) / 0.35850],
        1.0 / 2048.0,
    )
}

pub(super) fn convert_qt_matrix_trc(
    px: &[u8],
    layout: PixelLayout,
    p: &[u8],
    tags: &BTreeMap<[u8; 4], &[u8]>,
) -> Result<(IccOutcome, Vec<u8>), IccOutcome> {
    let (gray, cls) = (&p[16..20] == b"GRAY", &p[12..16]);
    let cls_ok = if gray {
        matches!(cls, b"mntr" | b"scnr" | b"prtr")
    } else {
        matches!(cls, b"mntr" | b"scnr")
    };
    if !cls_ok {
        return Err(IccOutcome::UnsupportedFallback);
    }
    let xyz_t = |k: &[u8; 4]| {
        tags.get(k)
            .filter(|t| t.len() >= 20 && &t[..4] == b"XYZ ")
            .map(|t| s15_xyz(&t[8..20]))
            .ok_or(IccOutcome::MalformedFallback)
    };
    let crv_t = |k: &[u8; 4]| {
        tags.get(k)
            .and_then(|t| parse_qt_curve(t))
            .ok_or(IccOutcome::MalformedFallback)
    };

    let (matrix, curves) = if gray {
        let w = xyz_t(b"wtpt")?;
        if (w[1] - 1.0).abs() > 1e-5 || (1.0 + w[2]) + w[0] == 0.0 {
            return Err(IccOutcome::MalformedFallback);
        }
        let d65 = [0.31271 / 0.32902, 1.0, (1.0 - 0.31271 - 0.32902) / 0.32902];
        let mat = if tol3(w, d65, 1.0 / 2048.0) {
            primary_matrix(PRIMARIES[0])
        } else {
            let y = 1.0 / ((1.0 + w[2]) + w[0]);
            let x = w[0] * y;
            if !(0.0..=1.0).contains(&x) || !(y > 0.0 && y <= 1.0) || (x as f64 + y as f64) > 1.0 {
                return Err(IccOutcome::MalformedFallback);
            }
            primary_matrix([PRIMARIES[0][0], PRIMARIES[0][1], PRIMARIES[0][2], (x, y)])
        };
        let c = crv_t(b"kTRC")?;
        (mat, [c.clone(), c.clone(), c])
    } else {
        let _ = xyz_t(b"wtpt")?;
        let mut mat = [xyz_t(b"rXYZ")?, xyz_t(b"gXYZ")?, xyz_t(b"bXYZ")?];
        for (pts, known) in PRIMARIES.iter().zip(RECOGNIZED.iter()) {
            if tol_mat(mat, *known, 1.0 / 2048.0) {
                mat = primary_matrix(*pts);
                break;
            }
        }
        let has_apple = [b"aarg", b"aagg", b"aabg"]
            .iter()
            .all(|k| tags.contains_key(*k));
        let ks = if has_apple {
            [*b"aarg", *b"aagg", *b"aabg"]
        } else {
            [*b"rTRC", *b"gTRC", *b"bTRC"]
        };
        (mat, [crv_t(&ks[0])?, crv_t(&ks[1])?, crv_t(&ks[2])?])
    };

    if mat_det(matrix).abs() <= 1e-5 {
        return Err(IccOutcome::MalformedFallback);
    }
    let curves = normalize_curves(curves);
    let target = primary_matrix(PRIMARIES[0]);
    let is_srgb = curves
        .iter()
        .all(|c| matches!(c, QtCurve::Func(f) if eq_slice(f, &SRGB_PARAMS, 1.0 / 512.0)));
    if tol_mat(matrix, target, 1.0 / 2048.0) && is_srgb {
        return Ok((IccOutcome::AlreadySrgb, Vec::new()));
    }
    let xf = mat_mul(mat_inv(target), matrix);
    if mat_det(xf).abs() <= 1e-5 {
        return Err(IccOutcome::MalformedFallback);
    }
    let t0 = build_src_lut(&curves[0]).ok_or(IccOutcome::MalformedFallback)?;
    let tables = if same_curve(&curves[0], &curves[1]) && same_curve(&curves[0], &curves[2]) {
        [t0, t0, t0]
    } else {
        let t1 = build_src_lut(&curves[1]).ok_or(IccOutcome::MalformedFallback)?;
        let t2 = build_src_lut(&curves[2]).ok_or(IccOutcome::MalformedFallback)?;
        [t0, t1, t2]
    };
    let ident = tol_mat(
        xf,
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        1.0 / 2048.0,
    );
    let (tgt_lut, inv_s) = (target_srgb_lut(), 1.0_f32 / 65_280.0_f32);
    let map_rgb = |[r, g, b]: [u8; 3]| -> Result<[u8; 3], IccOutcome> {
        let mut lin = [
            tables[0][r as usize] as f32 * inv_s,
            tables[1][g as usize] as f32 * inv_s,
            tables[2][b as usize] as f32 * inv_s,
        ];
        if !ident {
            let m = map_vec(xf, lin);
            lin = [
                m[0].clamp(0.0, 1.0),
                m[1].clamp(0.0, 1.0),
                m[2].clamp(0.0, 1.0),
            ];
        }
        let mut o = [0_u8; 3];
        for c in 0..3 {
            let idx = (lin[c] * 4_080.0_f32).round_ties_even() as i32;
            let u = usize::try_from(idx)
                .ok()
                .filter(|&i| i <= 4_080)
                .ok_or(IccOutcome::MalformedFallback)?;
            o[c] = ((tgt_lut[u] as u32 + 128) >> 8) as u8;
        }
        Ok(o)
    };
    let mut out = Vec::with_capacity(px.len());
    match layout {
        PixelLayout::Rgb8 => {
            for &c in px.as_chunks::<3>().0 {
                out.extend_from_slice(&map_rgb(c)?);
            }
        }
        PixelLayout::Rgba8 => {
            for &[r, g, b, a] in px.as_chunks::<4>().0 {
                let [or, og, ob] = map_rgb([r, g, b])?;
                out.extend_from_slice(&[or, og, ob, a]);
            }
        }
        PixelLayout::Gray8 => {
            for &g in px {
                out.push(qgray(map_rgb([g, g, g])?));
            }
        }
    }
    Ok((IccOutcome::Converted, out))
}

fn s15_xyz(b: &[u8]) -> [f32; 3] {
    [
        bei32(&b[0..4]) as f32 / 65_536.0,
        bei32(&b[4..8]) as f32 / 65_536.0,
        bei32(&b[8..12]) as f32 / 65_536.0,
    ]
}

fn map_vec(m: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [
        (v[0] * m[0][0] + v[1] * m[1][0]) + v[2] * m[2][0],
        (v[0] * m[0][1] + v[1] * m[1][1]) + v[2] * m[2][1],
        (v[0] * m[0][2] + v[1] * m[1][2]) + v[2] * m[2][2],
    ]
}

fn mat_mul(l: [[f32; 3]; 3], r: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    [map_vec(l, r[0]), map_vec(l, r[1]), map_vec(l, r[2])]
}

fn mat_det([r, g, b]: [[f32; 3]; 3]) -> f32 {
    (r[0] * (b[2] * g[1] - g[2] * b[1]) - r[1] * (b[2] * g[0] - g[2] * b[0]))
        + r[2] * (b[1] * g[0] - g[1] * b[0])
}

fn mat_inv(m: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let ([r, g, b], d) = (m, 1.0_f32 / mat_det(m));
    [
        [
            (g[1] * b[2] - b[1] * g[2]) * d,
            (b[1] * r[2] - r[1] * b[2]) * d,
            (r[1] * g[2] - g[1] * r[2]) * d,
        ],
        [
            (b[0] * g[2] - g[0] * b[2]) * d,
            (r[0] * b[2] - b[0] * r[2]) * d,
            (g[0] * r[2] - r[0] * g[2]) * d,
        ],
        [
            (g[0] * b[1] - b[0] * g[1]) * d,
            (b[0] * r[1] - r[0] * b[1]) * d,
            (r[0] * g[1] - g[0] * r[1]) * d,
        ],
    ]
}

fn primary_matrix(pts: [(f32, f32); 4]) -> [[f32; 3]; 3] {
    let pt = |(x, y): (f32, f32)| [x / y, 1.0, (1.0 - x - y) / y];
    let diag = |v: [f32; 3]| [[v[0], 0.0, 0.0], [0.0, v[1], 0.0], [0.0, 0.0, v[2]]];
    let (white, d50) = (pt(pts[3]), pt((0.34567, 0.35850)));
    let base = [pt(pts[0]), pt(pts[1]), pt(pts[2])];
    let mut matrix = mat_mul(base, diag(map_vec(mat_inv(base), white)));
    if !tol3(white, d50, 1.0 / 2048.0) {
        let brad = [
            [0.8951_f32, -0.7502, 0.0389],
            [0.2664, 1.7135, -0.0685],
            [-0.1614, 0.0367, 1.0296],
        ];
        let inv = [
            [0.986_992_9_f32, 0.432_305_3, -0.008_528_7],
            [-0.147_054_3, 0.518_360_3, 0.040_042_8],
            [0.159_962_7, 0.049_291_2, 0.968_486_7],
        ];
        let (num, den) = (map_vec(brad, d50), map_vec(brad, white));
        let scale = diag([num[0] / den[0], num[1] / den[1], num[2] / den[2]]);
        matrix = mat_mul(mat_mul(inv, mat_mul(scale, brad)), matrix);
    }
    matrix
}

fn tol3(a: [f32; 3], b: [f32; 3], t: f32) -> bool {
    (a[0] - b[0]).abs() < t && (a[1] - b[1]).abs() < t && (a[2] - b[2]).abs() < t
}

fn tol_mat(a: [[f32; 3]; 3], b: [[f32; 3]; 3], t: f32) -> bool {
    tol3(a[0], b[0], t) && tol3(a[1], b[1], t) && tol3(a[2], b[2], t)
}

fn eq_slice(a: &[f32], b: &[f32], t: f32) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() <= t)
}

fn same_curve(a: &QtCurve, b: &QtCurve) -> bool {
    match (a, b) {
        (QtCurve::Table(ta), QtCurve::Table(tb)) => ta == tb,
        (QtCurve::Func(fa), QtCurve::Func(fb)) => eq_slice(fa, fb, 1.0 / 512.0),
        _ => false,
    }
}

fn normalize_curves(curves: [QtCurve; 3]) -> [QtCurve; 3] {
    if let (QtCurve::Func(f0), QtCurve::Func(f1), QtCurve::Func(f2)) =
        (&curves[0], &curves[1], &curves[2])
        && eq_slice(f0, f1, 1.0 / 512.0)
        && eq_slice(f0, f2, 1.0 / 512.0)
    {
        let mut f = *f0;
        if eq_slice(
            &[f[0], f[1], f[3], f[4]],
            &[1.0, 0.0, 0.0, 0.0],
            1.0 / 512.0,
        ) {
            let g = if (f[6] - 1.0).abs() <= 1e-5 {
                1.0
            } else {
                f[6]
            };
            f = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, g];
        } else if eq_slice(&f, &SRGB_PARAMS, 1.0 / 512.0) {
            f = SRGB_PARAMS;
        }
        return [QtCurve::Func(f), QtCurve::Func(f), QtCurve::Func(f)];
    }
    curves
}

fn parse_qt_curve(tag: &[u8]) -> Option<QtCurve> {
    if tag.len() < 12 {
        return None;
    }
    if &tag[..4] == b"curv" {
        let count = be32(&tag[8..12]) as usize;
        if count > 65_536 || tag.len() < 12 + count * 2 {
            return None;
        }
        if count <= 1 {
            let g = if count == 0 {
                1.0
            } else {
                be16(&tag[12..14]) as f32 / 256.0
            };
            let c = if count == 0 { 1.0 } else { 0.0 };
            return Some(QtCurve::Func([1.0, 0.0, c, 0.0, 0.0, 0.0, g]));
        }
        let mut tbl = Vec::with_capacity(count);
        for i in 0..count {
            let v = be16(&tag[12 + i * 2..14 + i * 2]) as f32;
            if tbl.last().is_some_and(|&prev| v < prev) {
                return None;
            }
            tbl.push(v);
        }
        if tbl[0] == 0.0 && tbl[count - 1] == 65_535.0 {
            if count == 2 {
                return Some(QtCurve::Func([1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]));
            }
            let sh = (count == 26 && [tbl[6], tbl[12], tbl[18]] == [3062.0, 12824.0, 31237.0])
                || (count == 1024 && [tbl[257], tbl[513], tbl[768]] == [3366.0, 14116.0, 34318.0])
                || (count == 4096 && [tbl[515], tbl[1025], tbl[2051]] == [960.0, 3342.0, 14079.0]);
            if sh {
                return Some(QtCurve::Func(SRGB_PARAMS));
            }
        }
        return Some(QtCurve::Table(tbl));
    }
    if &tag[..4] != b"para" || tag.len() < 16 {
        return None;
    }
    let kind = be16(&tag[8..10]) as usize;
    let &needed = [1_usize, 3, 4, 5, 7]
        .get(kind)
        .filter(|&&n| tag.len() >= 12 + n * 4)?;
    let mut p = [0.0_f32; 7];
    for (i, s) in p.iter_mut().take(needed).enumerate() {
        *s = bei32(&tag[12 + i * 4..16 + i * 4]) as f32 / 65_536.0;
    }
    let c3 = if kind == 2 { p[3] } else { 0.0 };
    match kind {
        0 => Some(QtCurve::Func([1.0, 0.0, 0.0, 0.0, 0.0, 0.0, p[0]])),
        1 | 2 => {
            (p[1] != 0.0).then_some(QtCurve::Func([p[1], p[2], 0.0, -p[2] / p[1], c3, c3, p[0]]))
        }
        3 => Some(QtCurve::Func([p[1], p[2], p[3], p[4], 0.0, 0.0, p[0]])),
        _ => Some(QtCurve::Func([p[1], p[2], p[3], p[4], p[5], p[6], p[0]])),
    }
}

fn eval_qt_func(&[a, b, c, d, e, f, g]: &[f32; 7], x: f32) -> f32 {
    if x < d {
        c * x + f
    } else {
        (((a * x + b) as f64).powf(g as f64) as f32) + e
    }
}

fn build_src_lut(curve: &QtCurve) -> Option<[u16; 256]> {
    let mut lut = [0_u16; 256];
    match curve {
        QtCurve::Table(vals) => {
            let last = (vals.len() - 1) as f32;
            for (i, slot) in lut.iter_mut().enumerate() {
                let at = (i as f32 / 255.0) * last;
                let lo = at.floor() as usize;
                let (hi, frac) = ((lo + 1).min(vals.len() - 1), at - lo as f32);
                let y = (vals[lo] * (1.0 - frac) + vals[hi] * frac) * (1.0 / 65_535.0);
                *slot = (y * 65_280.0 + 0.5).floor().clamp(0.0, 65_280.0) as u16;
            }
        }
        QtCurve::Func(fun) => {
            for (i, slot) in lut.iter_mut().enumerate() {
                let y = eval_qt_func(fun, i as f32 / 255.0);
                if !y.is_finite() {
                    return None;
                }
                let s = y * 65_280.0;
                let nb = (s - 1.0).trunc();
                let r = if s < 0.0 {
                    (s - nb + 0.5).trunc() + nb
                } else {
                    (s + 0.5).trunc()
                };
                *slot = (r as i64).rem_euclid(65_536) as u16;
            }
        }
    }
    Some(lut)
}

fn target_srgb_lut() -> &'static [u16; 4081] {
    static LUT: OnceLock<[u16; 4081]> = OnceLock::new();
    LUT.get_or_init(|| {
        let [a, b, c, d, e, f, g] = SRGB_PARAMS;
        let ia = ((1.0_f32 / a) as f64).powf(g as f64) as f32;
        let inv = [ia, -ia * e, 1.0 / c, c * d + f, -b / a, -f / c, 1.0 / g];
        std::array::from_fn(|i| {
            (eval_qt_func(&inv, (i as f64 / 4_080.0) as f32) * 65_280.0 + 0.5).floor() as u16
        })
    })
}
