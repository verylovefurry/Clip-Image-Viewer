#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorkArea {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

pub const DEFAULT_WIDTH: f64 = 720.0;
pub const DEFAULT_HEIGHT: f64 = 960.0;
pub const MIN_WIDTH: f64 = 320.0;
pub const MIN_HEIGHT: f64 = 320.0;

impl WorkArea {
    fn valid(self) -> bool {
        [self.x, self.y, self.width, self.height]
            .into_iter()
            .all(f64::is_finite)
            && self.width > 0.0
            && self.height > 0.0
    }
}

pub fn create_default_bounds(work_area: WorkArea) -> Bounds {
    assert!(
        work_area.valid(),
        "work area must have finite positive dimensions"
    );
    let scale = 1.0_f64
        .min(work_area.width / DEFAULT_WIDTH)
        .min(work_area.height / DEFAULT_HEIGHT);
    let width = (DEFAULT_WIDTH * scale).round().max(1.0);
    let height = (DEFAULT_HEIGHT * scale).round().max(1.0);
    Bounds {
        x: work_area.x + ((work_area.width - width) / 2.0).round(),
        y: work_area.y + ((work_area.height - height) / 2.0).round(),
        width,
        height,
    }
}

/// Matches the JS restore rules; invalid dimensions fall back to the centered default.
pub fn constrain_bounds(bounds: Option<Bounds>, work_area: WorkArea) -> Bounds {
    assert!(
        work_area.valid(),
        "work area must have finite positive dimensions"
    );
    let Some(bounds) = bounds.filter(|b| b.width.is_finite() && b.height.is_finite()) else {
        return create_default_bounds(work_area);
    };
    let min_width = MIN_WIDTH.min(work_area.width);
    let min_height = MIN_HEIGHT.min(work_area.height);
    let width = work_area.width.min(min_width.max(bounds.width.round()));
    let height = work_area.height.min(min_height.max(bounds.height.round()));
    let fallback_x = work_area.x + ((work_area.width - width) / 2.0).round();
    let fallback_y = work_area.y + ((work_area.height - height) / 2.0).round();
    let x = if bounds.x.is_finite() {
        bounds.x.round()
    } else {
        fallback_x
    };
    let y = if bounds.y.is_finite() {
        bounds.y.round()
    } else {
        fallback_y
    };
    Bounds {
        x: x.max(work_area.x)
            .min(work_area.x + work_area.width - width),
        y: y.max(work_area.y)
            .min(work_area.y + work_area.height - height),
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_center_at_three_by_four_and_scale_down() {
        let area = WorkArea {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1040.0,
        };
        assert_eq!(
            create_default_bounds(area),
            Bounds {
                x: 600.0,
                y: 40.0,
                width: 720.0,
                height: 960.0
            }
        );
        let small = create_default_bounds(WorkArea {
            x: -1280.0,
            y: 0.0,
            width: 1280.0,
            height: 720.0,
        });
        assert!(small.width <= 1280.0 && small.height <= 720.0);
        assert!((small.width / small.height - 0.75).abs() < 0.002);
    }
    #[test]
    fn restored_bounds_are_constrained_to_work_area() {
        let area = WorkArea {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1040.0,
        };
        assert_eq!(
            constrain_bounds(
                Some(Bounds {
                    x: 140.0,
                    y: 90.0,
                    width: 1100.0,
                    height: 760.0
                }),
                area
            ),
            Bounds {
                x: 140.0,
                y: 90.0,
                width: 1100.0,
                height: 760.0
            }
        );
        assert_eq!(
            constrain_bounds(
                Some(Bounds {
                    x: 5000.0,
                    y: -900.0,
                    width: 2600.0,
                    height: 1500.0
                }),
                area
            ),
            Bounds {
                x: 0.0,
                y: 0.0,
                width: 1920.0,
                height: 1040.0
            }
        );
        assert_eq!(constrain_bounds(None, area), create_default_bounds(area));
    }
    #[test]
    fn minimums_and_nonfinite_position_match_js_fallback() {
        let area = WorkArea {
            x: -1000.0,
            y: 50.0,
            width: 500.0,
            height: 400.0,
        };
        assert_eq!(
            constrain_bounds(
                Some(Bounds {
                    x: f64::NAN,
                    y: f64::INFINITY,
                    width: 1.0,
                    height: 2.0
                }),
                area
            ),
            Bounds {
                x: -910.0,
                y: 90.0,
                width: 320.0,
                height: 320.0
            }
        );
    }
}
