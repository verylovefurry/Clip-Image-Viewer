#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FitInput {
    pub natural_width: f64,
    pub natural_height: f64,
    pub viewport: Rect,
    pub rotation_degrees: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub zoom: f64,
    pub pan_x: f64,
    pub pan_y: f64,
    pub fit_mode: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewportError {
    NonFiniteInput,
}

/// Converts a client-space pointer to image pixels after reversing the viewer transform.
pub fn image_point(pointer: Point, viewport: Rect, size: Point, pan: Point, zoom: f64, rotation: f64, flip: f64) -> Option<Point> {
    if ![pointer.x, pointer.y, viewport.left, viewport.top, viewport.width, viewport.height,
        size.x, size.y, pan.x, pan.y, zoom, rotation, flip].into_iter().all(f64::is_finite)
        || size.x <= 0.0 || size.y <= 0.0 || zoom <= 0.0 || flip == 0.0 { return None; }
    let x = pointer.x - (viewport.left + viewport.width / 2.0 + pan.x);
    let y = pointer.y - (viewport.top + viewport.height / 2.0 + pan.y);
    let angle = -rotation.to_radians();
    let (sin, cos) = angle.sin_cos();
    let point = Point { x: (x * cos - y * sin) / (zoom * flip) + size.x / 2.0,
        y: (x * sin + y * cos) / zoom + size.y / 2.0 };
    (point.x >= 0.0 && point.y >= 0.0 && point.x < size.x && point.y < size.y).then_some(point)
}

/// Computes the app's fit zoom, including quarter-turn dimension swapping.
pub fn calculate_fit_zoom(input: FitInput) -> Result<f64, ViewportError> {
    let values = [
        input.natural_width,
        input.natural_height,
        input.viewport.left,
        input.viewport.top,
        input.viewport.width,
        input.viewport.height,
        input.rotation_degrees,
    ];
    if !values.into_iter().all(f64::is_finite) {
        return Err(ViewportError::NonFiniteInput);
    }
    if input.natural_width <= 0.0 || input.natural_height <= 0.0 {
        return Ok(1.0);
    }
    let quarter_turn = (input.rotation_degrees % 180.0).abs() == 90.0;
    let (width, height) = if quarter_turn {
        (input.natural_height, input.natural_width)
    } else {
        (input.natural_width, input.natural_height)
    };
    let available_width = input.viewport.width.max(1.0);
    let available_height = input.viewport.height.max(1.0);
    Ok((available_width / width).min(available_height / height))
}

impl Viewport {
    pub const MIN_ZOOM: f64 = 0.02;
    pub const MAX_ZOOM: f64 = 32.0;

    /// Applies multiplicative zoom around a client-space pointer inside the viewport.
    /// Pointer zoom keeps the same screen-space image point stationary, matching the renderer.
    pub fn zoom_at(
        &mut self,
        factor: f64,
        pointer: Option<Point>,
        rect: Rect,
        is_image: bool,
        dragging: bool,
        pan_start: &mut Point,
    ) -> Result<(), ViewportError> {
        if !factor.is_finite()
            || !self.zoom.is_finite()
            || !self.pan_x.is_finite()
            || !self.pan_y.is_finite()
            || ![
                rect.left,
                rect.top,
                rect.width,
                rect.height,
                pan_start.x,
                pan_start.y,
            ]
            .into_iter()
            .all(f64::is_finite)
            || pointer.is_some_and(|p| !p.x.is_finite() || !p.y.is_finite())
        {
            return Err(ViewportError::NonFiniteInput);
        }
        let zoom = (self.zoom * factor).clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
        if is_image {
            if let Some(pointer) = pointer.filter(|p| {
                p.x >= rect.left
                    && p.x <= rect.left + rect.width
                    && p.y >= rect.top
                    && p.y <= rect.top + rect.height
            }) {
                let x = pointer.x - rect.left - rect.width / 2.0;
                let y = pointer.y - rect.top - rect.height / 2.0;
                let ratio = zoom / self.zoom;
                let next_x = x - (x - self.pan_x) * ratio;
                let next_y = y - (y - self.pan_y) * ratio;
                if dragging {
                    pan_start.x += next_x - self.pan_x;
                    pan_start.y += next_y - self.pan_y;
                }
                self.pan_x = next_x;
                self.pan_y = next_y;
            }
        }
        self.fit_mode = false;
        self.zoom = zoom;
        Ok(())
    }

    pub fn fit(&mut self, zoom: f64) -> Result<(), ViewportError> {
        if !zoom.is_finite() {
            return Err(ViewportError::NonFiniteInput);
        }
        self.zoom = zoom;
        self.pan_x = 0.0;
        self.pan_y = 0.0;
        self.fit_mode = true;
        Ok(())
    }

    pub fn actual_size(&mut self) {
        self.zoom = 1.0;
        self.pan_x = 0.0;
        self.pan_y = 0.0;
        self.fit_mode = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picking_reverses_zoom_pan_rotation_and_flip() {
        let rect = Rect { left: 10., top: 20., width: 200., height: 100. };
        let size = Point { x: 100., y: 80. };
        let pan = Point { x: 5., y: -10. };
        let center = Point { x: 115., y: 60. };
        assert_eq!(image_point(center, rect, size, pan, 2., 0., 1.), Some(Point { x: 50., y: 40. }));
        let picked = image_point(Point { x: 115., y: 40. }, rect, size, pan, 2., 90., -1.).unwrap();
        assert!((picked.x - 60.).abs() < 1e-9 && (picked.y - 40.).abs() < 1e-9);
        assert!(image_point(Point { x: 1000., y: 1000. }, rect, size, pan, 1., 0., 1.).is_none());
        assert!(image_point(center, rect, size, pan, 0., 0., 1.).is_none());
    }
    fn rect() -> Rect {
        Rect {
            left: 100.0,
            top: 50.0,
            width: 800.0,
            height: 600.0,
        }
    }
    #[test]
    fn fit_zoom_swaps_dimensions_on_quarter_turn() {
        let input = FitInput {
            natural_width: 1200.0,
            natural_height: 800.0,
            viewport: rect(),
            rotation_degrees: 0.0,
        };
        assert_eq!(calculate_fit_zoom(input), Ok(2.0 / 3.0));
        assert_eq!(
            calculate_fit_zoom(FitInput {
                rotation_degrees: 90.0,
                ..input
            }),
            Ok(0.5)
        );
        assert_eq!(
            calculate_fit_zoom(FitInput {
                natural_width: 0.0,
                ..input
            }),
            Ok(1.0)
        );
    }
    #[test]
    fn zoom_keeps_pointer_anchor_and_clamps_limits() {
        let mut state = Viewport {
            zoom: 1.0,
            pan_x: 20.0,
            pan_y: -10.0,
            fit_mode: true,
        };
        let mut start = Point { x: 0.0, y: 0.0 };
        state
            .zoom_at(
                2.0,
                Some(Point { x: 250.0, y: 180.0 }),
                rect(),
                true,
                false,
                &mut start,
            )
            .unwrap();
        assert_eq!(state.pan_x, 290.0);
        assert_eq!(state.pan_y, 150.0);
        assert!(!state.fit_mode);
        state
            .zoom_at(100.0, None, rect(), true, false, &mut start)
            .unwrap();
        assert_eq!(state.zoom, 32.0);
        state
            .zoom_at(0.0, None, rect(), true, false, &mut start)
            .unwrap();
        assert_eq!(state.zoom, 0.02);
    }
    #[test]
    fn zoom_during_drag_preserves_drag_baseline_and_outside_pointer_centers() {
        let mut state = Viewport {
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            fit_mode: false,
        };
        let mut start = Point { x: 12.0, y: 34.0 };
        state
            .zoom_at(
                2.0,
                Some(Point { x: 0.0, y: 0.0 }),
                rect(),
                true,
                true,
                &mut start,
            )
            .unwrap();
        assert_eq!((state.pan_x, state.pan_y), (0.0, 0.0));
        state
            .zoom_at(
                2.0,
                Some(Point { x: 500.0, y: 300.0 }),
                rect(),
                true,
                true,
                &mut start,
            )
            .unwrap();
        assert_eq!((start.x, start.y), (12.0, 84.0));
        assert_eq!((state.pan_x, state.pan_y), (0.0, 50.0));
    }
    #[test]
    fn video_and_nonfinite_inputs_are_handled_explicitly() {
        let mut state = Viewport {
            zoom: 1.0,
            pan_x: 4.0,
            pan_y: 5.0,
            fit_mode: false,
        };
        let mut start = Point { x: 0.0, y: 0.0 };
        state
            .zoom_at(
                2.0,
                Some(Point { x: 500.0, y: 300.0 }),
                rect(),
                false,
                false,
                &mut start,
            )
            .unwrap();
        assert_eq!((state.pan_x, state.pan_y), (4.0, 5.0));
        assert_eq!(
            state.zoom_at(f64::NAN, None, rect(), true, false, &mut start),
            Err(ViewportError::NonFiniteInput)
        );
    }
}
