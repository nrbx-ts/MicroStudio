// f64 components (not f32); equality is exact

mod cframe;
mod color;
mod misc;
mod ui;
mod vector;

pub use cframe::{CFrame, EulerAngles};
pub use color::{Color3, Color3Hsv};
pub use misc::{BrickColor, Ray};
pub use ui::{Rect, UDim, UDim2};
pub use vector::{Vector2, Vector3};

// roblox's "close enough" tolerance
#[inline]
pub fn fuzzy_eq(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-5
}
