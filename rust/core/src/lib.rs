//! Deterministic, dependency-free native primitives for Clip Image Viewer.

pub mod compositor;
pub mod viewport;
pub mod window_state;

pub use compositor::{blend_color, composite_rgba, BlendMode, CompositorError};
pub use viewport::{calculate_fit_zoom, image_point, FitInput, Point, Rect, Viewport};
pub use window_state::{constrain_bounds, create_default_bounds, Bounds, WorkArea};
