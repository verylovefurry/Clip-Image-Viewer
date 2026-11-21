use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlendMode(u8);

impl BlendMode {
    pub fn new(code: u8) -> Result<Self, CompositorError> {
        if (code <= 26) || code == 30 || code == 36 {
            Ok(Self(code))
        } else {
            Err(CompositorError::UnsupportedMode(code))
        }
    }

    pub fn code(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CompositorError {
    UnsupportedMode(u8),
    LengthMismatch,
    InvalidLength,
    NonFiniteInput,
}

impl fmt::Display for CompositorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedMode(mode) => write!(f, "unsupported CLIP blend mode: {mode}"),
            Self::LengthMismatch => f.write_str("CLIP composite buffer lengths differ"),
            Self::InvalidLength => f.write_str("RGBA buffer length must be divisible by four"),
            Self::NonFiniteInput => f.write_str("compositor input must be finite"),
        }
    }
}

impl std::error::Error for CompositorError {}

fn clamp(value: f64) -> f64 {
    value.max(0.0).min(1.0)
}

fn byte(value: f64) -> u8 {
    let bounded = value.max(0.0).min(255.0);
    let integer = bounded.floor();
    let fraction = bounded - integer;
    let rounded = if fraction == 0.5 {
        integer + ((integer as u64) & 1) as f64
    } else {
        bounded.round()
    };
    rounded as u8
}

fn luminance(color: [f64; 3]) -> f64 {
    color[0] * 0.3 + color[1] * 0.59 + color[2] * 0.11
}
fn saturation(color: [f64; 3]) -> f64 {
    color.iter().copied().fold(f64::NEG_INFINITY, f64::max)
        - color.iter().copied().fold(f64::INFINITY, f64::min)
}

fn set_luminance(color: [f64; 3], target: f64) -> [f64; 3] {
    let difference = target - luminance(color);
    let mut result = color.map(|channel| channel + difference);
    let low = result.iter().copied().fold(f64::INFINITY, f64::min);
    let high = result.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if low < 0.0 {
        result = result.map(|channel| target + (channel - target) * target / (target - low));
    }
    if high > 1.0 {
        result =
            result.map(|channel| target + (channel - target) * (1.0 - target) / (high - target));
    }
    result
}

fn set_saturation(color: [f64; 3], target: f64) -> [f64; 3] {
    let low = color.iter().copied().fold(f64::INFINITY, f64::min);
    let range = color.iter().copied().fold(f64::NEG_INFINITY, f64::max) - low;
    if range != 0.0 {
        color.map(|channel| (channel - low) * target / range)
    } else {
        [0.0; 3]
    }
}

fn burn(backdrop: f64, source: f64) -> f64 {
    if source <= 0.0 {
        0.0
    } else {
        1.0 - ((1.0 - backdrop) / source).min(1.0)
    }
}
fn dodge(backdrop: f64, source: f64) -> f64 {
    if source >= 1.0 {
        1.0
    } else {
        (backdrop / (1.0 - source)).min(1.0)
    }
}
fn hard_light(backdrop: f64, source: f64) -> f64 {
    if source <= 0.5 {
        2.0 * backdrop * source
    } else {
        1.0 - 2.0 * (1.0 - backdrop) * (1.0 - source)
    }
}

fn blend_channel(mode: u8, backdrop: f64, source: f64) -> f64 {
    match mode {
        0 | 30 => source,
        1 => backdrop.min(source),
        2 => backdrop * source,
        3 => burn(backdrop, source),
        4 => clamp(backdrop + source - 1.0),
        5 => (backdrop - source).max(0.0),
        7 => backdrop.max(source),
        8 => backdrop + source - backdrop * source,
        9 | 10 => dodge(backdrop, source),
        11 | 12 => (backdrop + source).min(1.0),
        14 => hard_light(source, backdrop),
        15 => {
            let curve = if backdrop <= 0.25 {
                ((16.0 * backdrop - 12.0) * backdrop + 4.0) * backdrop
            } else {
                backdrop.sqrt()
            };
            if source <= 0.5 {
                backdrop - (1.0 - 2.0 * source) * backdrop * (1.0 - backdrop)
            } else {
                backdrop + (2.0 * source - 1.0) * (curve - backdrop)
            }
        }
        16 => hard_light(backdrop, source),
        17 => {
            if source <= 0.5 {
                burn(backdrop, source * 2.0)
            } else {
                dodge(backdrop, (source - 0.5) * 2.0)
            }
        }
        18 => clamp(backdrop + 2.0 * source - 1.0),
        19 => {
            if source <= 0.5 {
                backdrop.min(source * 2.0)
            } else {
                backdrop.max((source - 0.5) * 2.0)
            }
        }
        20 => {
            if backdrop + source >= 1.0 {
                1.0
            } else {
                0.0
            }
        }
        21 => (backdrop - source).abs(),
        22 => backdrop + source - 2.0 * backdrop * source,
        36 => (backdrop / source.max(1e-9)).min(1.0),
        _ => unreachable!("mode validated by BlendMode"),
    }
}

pub fn blend_color(
    mode: BlendMode,
    backdrop: [f64; 3],
    source: [f64; 3],
) -> Result<[f64; 3], CompositorError> {
    if !backdrop.into_iter().chain(source).all(f64::is_finite) {
        return Err(CompositorError::NonFiniteInput);
    }
    let code = mode.code();
    let color = match code {
        6 => {
            if luminance(source) < luminance(backdrop) {
                source
            } else {
                backdrop
            }
        }
        13 => {
            if luminance(source) > luminance(backdrop) {
                source
            } else {
                backdrop
            }
        }
        23 => set_luminance(
            set_saturation(source, saturation(backdrop)),
            luminance(backdrop),
        ),
        24 => set_luminance(
            set_saturation(backdrop, saturation(source)),
            luminance(backdrop),
        ),
        25 => set_luminance(source, luminance(backdrop)),
        26 => set_luminance(backdrop, luminance(source)),
        _ => std::array::from_fn(|index| blend_channel(code, backdrop[index], source[index])),
    };
    if color.into_iter().all(f64::is_finite) {
        Ok(color)
    } else {
        Err(CompositorError::NonFiniteInput)
    }
}

/// Composites source RGBA bytes into destination in place; source is never modified.
pub fn composite_rgba(
    destination: &mut [u8],
    source: &[u8],
    mode: BlendMode,
    opacity: f64,
    clipped: bool,
) -> Result<(), CompositorError> {
    if destination.len() != source.len() {
        return Err(CompositorError::LengthMismatch);
    }
    if destination.len() % 4 != 0 {
        return Err(CompositorError::InvalidLength);
    }
    if !opacity.is_finite() {
        return Err(CompositorError::NonFiniteInput);
    }
    let strength = clamp(opacity);
    if strength == 0.0 {
        return Ok(());
    }
    let code = mode.code();
    let glow = code == 10 || code == 12;
    let nonseparable = matches!(code, 6 | 13 | 23 | 24 | 25 | 26);
    for (dst, src) in destination.chunks_exact_mut(4).zip(source.chunks_exact(4)) {
        let source_alpha = src[3] as f64 / 255.0 * strength;
        if source_alpha == 0.0 {
            continue;
        }
        let backdrop_alpha = dst[3] as f64 / 255.0;
        if clipped && backdrop_alpha == 0.0 {
            continue;
        }
        if !clipped && matches!(code, 0 | 30) && source_alpha == 1.0 {
            dst.copy_from_slice(src);
            continue;
        }
        let output_alpha = if clipped {
            backdrop_alpha
        } else {
            source_alpha + backdrop_alpha * (1.0 - source_alpha)
        };
        let mixed = if nonseparable {
            Some(blend_color(
                mode,
                [
                    dst[0] as f64 / 255.0,
                    dst[1] as f64 / 255.0,
                    dst[2] as f64 / 255.0,
                ],
                [
                    src[0] as f64 / 255.0,
                    src[1] as f64 / 255.0,
                    src[2] as f64 / 255.0,
                ],
            )?)
        } else {
            None
        };
        for channel in 0..3 {
            let backdrop = dst[channel] as f64 / 255.0;
            let foreground = src[channel] as f64 / 255.0;
            let blend = mixed.map_or_else(
                || {
                    blend_channel(
                        code,
                        backdrop,
                        if glow {
                            foreground * source_alpha
                        } else {
                            foreground
                        },
                    )
                },
                |rgb| rgb[channel],
            );
            let result = if clipped {
                if glow {
                    blend
                } else {
                    backdrop * (1.0 - source_alpha) + blend * source_alpha
                }
            } else if glow {
                ((1.0 - backdrop_alpha) * source_alpha * foreground + backdrop_alpha * blend)
                    / output_alpha
            } else {
                ((1.0 - backdrop_alpha) * source_alpha * foreground
                    + backdrop_alpha * source_alpha * blend
                    + (1.0 - source_alpha) * backdrop_alpha * backdrop)
                    / output_alpha
            };
            dst[channel] = byte(result * 255.0);
        }
        dst[3] = byte(output_alpha * 255.0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_reference_modes_and_all_modes_are_valid() {
        let backdrop = [64, 128, 192, 255];
        let source = [128, 192, 64, 255];
        for (mode, expected) in [
            (0, [128, 192, 64, 255]),
            (1, [64, 128, 64, 255]),
            (2, [32, 96, 48, 255]),
            (4, [0, 65, 1, 255]),
            (5, [0, 0, 128, 255]),
            (7, [128, 192, 192, 255]),
            (8, [160, 224, 208, 255]),
            (11, [192, 255, 255, 255]),
            (12, [192, 255, 255, 255]),
            (21, [64, 64, 128, 255]),
            (30, [128, 192, 64, 255]),
        ] {
            let mut output = backdrop;
            composite_rgba(
                &mut output,
                &source,
                BlendMode::new(mode).unwrap(),
                1.0,
                false,
            )
            .unwrap();
            assert_eq!(output, expected, "mode {mode}");
        }
        for mode in (0..=26).chain([30, 36]) {
            let parsed = BlendMode::new(mode).unwrap();
            let color = blend_color(parsed, [0.2, 0.5, 0.8], [0.8, 0.1, 0.4]).unwrap();
            assert!(color
                .into_iter()
                .all(|v| v.is_finite() && (-1e-9..=1.0 + 1e-9).contains(&v)));
            assert!(BlendMode::new(mode + 64).is_err());
        }
    }

    #[test]
    fn alpha_clipping_glow_and_even_rounding_match_js() {
        let mut pixel = [0, 0, 0, 0];
        composite_rgba(
            &mut pixel,
            &[200, 100, 20, 253],
            BlendMode::new(0).unwrap(),
            128.0 / 256.0,
            false,
        )
        .unwrap();
        assert_eq!(pixel, [200, 100, 20, 126]);
        let mut pixel = [10, 20, 30, 128];
        composite_rgba(
            &mut pixel,
            &[100, 200, 100, 255],
            BlendMode::new(0).unwrap(),
            1.0,
            true,
        )
        .unwrap();
        assert_eq!(pixel, [100, 200, 100, 128]);
        let mut pixel = [64, 64, 64, 255];
        composite_rgba(
            &mut pixel,
            &[255, 255, 255, 128],
            BlendMode::new(12).unwrap(),
            1.0,
            false,
        )
        .unwrap();
        assert_eq!(pixel, [192, 192, 192, 255]);
    }

    #[test]
    fn rejects_bad_lengths_modes_and_nonfinite_values_without_touching_source() {
        let src = [1, 2, 3, 4];
        let mut dst = [8, 9, 10, 11];
        assert_eq!(
            composite_rgba(&mut dst, &src[..3], BlendMode::new(0).unwrap(), 1.0, false),
            Err(CompositorError::LengthMismatch)
        );
        assert_eq!(
            composite_rgba(
                &mut dst[..3],
                &src[..3],
                BlendMode::new(0).unwrap(),
                1.0,
                false
            ),
            Err(CompositorError::InvalidLength)
        );
        assert!(BlendMode::new(99).is_err());
        assert_eq!(
            composite_rgba(&mut dst, &src, BlendMode::new(0).unwrap(), f64::NAN, false),
            Err(CompositorError::NonFiniteInput)
        );
        assert_eq!(
            blend_color(
                BlendMode::new(0).unwrap(),
                [0.0, f64::INFINITY, 0.0],
                [0.0; 3]
            ),
            Err(CompositorError::NonFiniteInput)
        );
        assert_eq!(src, [1, 2, 3, 4]);
    }
}
