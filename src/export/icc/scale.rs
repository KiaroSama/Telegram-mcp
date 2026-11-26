//! Qt `qSmoothScaleImage` integer separable box/bilinear resampling for Desktop thumbnails.

use super::{IccError, PixelLayout, validate_buf};

pub type ScaleTaps = Vec<(usize, Vec<i64>)>;

pub fn down_taps(source: u32, target: u32) -> ScaleTaps {
    if source == 0 || target == 0 {
        return Vec::new();
    }
    let (src, dst) = (source as i64, target as i64);
    let (inc, cp, mut val) = ((src << 16) / dst, ((dst << 14) + src - 1) / src, 0_i64);
    (0..target)
        .map(|_| {
            let ap = ((0x10000 - (val & 0xFFFF)) * cp) >> 16;
            let (mut w, mut rest, start) = (vec![ap], (1_i64 << 14) - ap, (val >> 16) as usize);
            while rest > cp {
                w.push(cp);
                rest -= cp;
            }
            w.push(rest);
            val += inc;
            (start, w)
        })
        .collect()
}

pub fn up_taps(source: u32, target: u32) -> ScaleTaps {
    if source == 0 || target == 0 {
        return Vec::new();
    }
    let (src, dst) = (source as i64, target as i64);
    let (inc, mut val) = ((src << 16) / dst, (0x8000 * src) / dst - 0x8000);
    (0..target)
        .map(|_| {
            let pos = val >> 16;
            let w = if pos < 0 || pos >= src - 1 {
                0
            } else {
                (val >> 8) & 0xFF
            };
            val += inc;
            (pos.max(0) as usize, vec![256 - w, w])
        })
        .collect()
}

pub fn smooth_scale(
    px: &[u8],
    src_w: u32,
    src_h: u32,
    layout: PixelLayout,
    dst_w: u32,
    dst_h: u32,
) -> Result<(Vec<u8>, PixelLayout), IccError> {
    validate_buf(px, src_w, src_h, layout)?;
    if dst_w == 0 || dst_h == 0 {
        return Err(IccError::InvalidDimensions {
            width: dst_w,
            height: dst_h,
        });
    }
    if src_w == dst_w && src_h == dst_h {
        return Ok((px.to_vec(), layout));
    }
    let (work, out_l) = match layout {
        PixelLayout::Gray8 => (
            px.iter().flat_map(|&g| [g as i64; 3]).collect::<Vec<_>>(),
            PixelLayout::Rgb8,
        ),
        _ => (px.iter().map(|&b| b as i64).collect(), layout),
    };
    let (ch, sw, sh, dw, dh) = (
        out_l.channels(),
        src_w as usize,
        src_h as usize,
        dst_w as usize,
        dst_h as usize,
    );
    let shr = |mut v: Vec<i64>, b: u32| {
        v.iter_mut().for_each(|x| *x >>= b);
        v
    };
    let scaled = match (dw >= sw, dh >= sh) {
        (false, false) => {
            let hx = shr(
                tap_axis(&work, sw, sh, ch, &down_taps(src_w, dst_w), true),
                4,
            );
            shr(
                tap_axis(&hx, dw, sh, ch, &down_taps(src_h, dst_h), false),
                24,
            )
        }
        (true, false) => {
            let vy = tap_axis(&work, sw, sh, ch, &down_taps(src_h, dst_h), false);
            shr(
                shr(tap_axis(&vy, sw, dh, ch, &up_taps(src_w, dst_w), true), 8),
                14,
            )
        }
        (false, true) => {
            let hx = tap_axis(&work, sw, sh, ch, &down_taps(src_w, dst_w), true);
            shr(
                shr(tap_axis(&hx, dw, sh, ch, &up_taps(src_h, dst_h), false), 8),
                14,
            )
        }
        (true, true) => {
            let vy = shr(
                tap_axis(&work, sw, sh, ch, &up_taps(src_h, dst_h), false),
                8,
            );
            shr(tap_axis(&vy, sw, dh, ch, &up_taps(src_w, dst_w), true), 8)
        }
    };
    Ok((
        scaled.into_iter().map(|v| v.clamp(0, 255) as u8).collect(),
        out_l,
    ))
}

fn tap_axis(
    src: &[i64],
    sw: usize,
    sh: usize,
    ch: usize,
    taps: &ScaleTaps,
    x_axis: bool,
) -> Vec<i64> {
    if x_axis {
        let (dw, mut dst) = (taps.len(), vec![0_i64; sh * taps.len() * ch]);
        for y in 0..sh {
            let rin = &src[y * sw * ch..(y + 1) * sw * ch];
            let rout = &mut dst[y * dw * ch..(y + 1) * dw * ch];
            for (x, (st, ws)) in taps.iter().enumerate() {
                for (k, &w) in ws.iter().enumerate() {
                    let s = (st + k).min(sw - 1) * ch;
                    for c in 0..ch {
                        rout[x * ch + c] += rin[s + c] * w;
                    }
                }
            }
        }
        dst
    } else {
        let (stride, mut dst) = (sw * ch, vec![0_i64; taps.len() * sw * ch]);
        for (y, (st, ws)) in taps.iter().enumerate() {
            let rout = &mut dst[y * stride..(y + 1) * stride];
            for (k, &w) in ws.iter().enumerate() {
                let s = (st + k).min(sh - 1) * stride;
                for (o, &i) in rout.iter_mut().zip(&src[s..s + stride]) {
                    *o += i * w;
                }
            }
        }
        dst
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_qt_taps_and_scales_rgb_and_gray() {
        assert_eq!(
            down_taps(3, 2),
            vec![(0, vec![10923, 5461]), (1, vec![5461, 10923])]
        );
        assert_eq!(
            up_taps(2, 4),
            vec![
                (0, vec![256, 0]),
                (0, vec![192, 64]),
                (0, vec![64, 192]),
                (1, vec![256, 0])
            ]
        );
        let (rgb, l) = smooth_scale(
            &[0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3],
            2,
            2,
            PixelLayout::Rgb8,
            1,
            1,
        )
        .expect("scale");
        assert_eq!((rgb, l), (vec![1, 1, 1], PixelLayout::Rgb8));
        let (gr, gl) = smooth_scale(&[10, 90], 2, 1, PixelLayout::Gray8, 1, 1).expect("gray");
        assert_eq!((gr, gl), (vec![50, 50, 50], PixelLayout::Rgb8));
    }
}
