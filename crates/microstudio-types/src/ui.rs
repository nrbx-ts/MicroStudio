use serde::{Deserialize, Serialize};
use std::ops::{Add, Sub};

use crate::Vector2;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct UDim {
    pub scale: f64,
    pub offset: i32,
}

impl UDim {
    #[inline]
    pub const fn new(scale: f64, offset: i32) -> Self {
        Self { scale, offset }
    }

    #[inline]
    pub fn resolve(&self, parent: f64) -> f64 {
        self.scale * parent + self.offset as f64
    }

    #[inline]
    pub fn lerp(&self, other: &UDim, alpha: f64) -> UDim {
        UDim::new(
            self.scale + (other.scale - self.scale) * alpha,
            (self.offset as f64 + (other.offset - self.offset) as f64 * alpha).round() as i32,
        )
    }
}

impl Add for UDim {
    type Output = UDim;
    #[inline]
    fn add(self, rhs: UDim) -> UDim {
        UDim::new(self.scale + rhs.scale, self.offset + rhs.offset)
    }
}

impl Sub for UDim {
    type Output = UDim;
    #[inline]
    fn sub(self, rhs: UDim) -> UDim {
        UDim::new(self.scale - rhs.scale, self.offset - rhs.offset)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct UDim2 {
    pub x: UDim,
    pub y: UDim,
}

impl UDim2 {
    #[inline]
    pub const fn new(x_scale: f64, x_offset: i32, y_scale: f64, y_offset: i32) -> Self {
        Self {
            x: UDim::new(x_scale, x_offset),
            y: UDim::new(y_scale, y_offset),
        }
    }

    #[inline]
    pub const fn from_scale(x_scale: f64, y_scale: f64) -> Self {
        Self::new(x_scale, 0, y_scale, 0)
    }

    #[inline]
    pub const fn from_offset(x_offset: i32, y_offset: i32) -> Self {
        Self::new(0.0, x_offset, 0.0, y_offset)
    }

    #[inline]
    pub const fn zero() -> Self {
        Self::new(0.0, 0, 0.0, 0)
    }

    #[inline]
    pub const fn from_x_y(x: UDim, y: UDim) -> Self {
        Self { x, y }
    }

    #[inline]
    pub fn lerp(&self, other: &UDim2, alpha: f64) -> UDim2 {
        UDim2 {
            x: self.x.lerp(&other.x, alpha),
            y: self.y.lerp(&other.y, alpha),
        }
    }

    #[inline]
    pub fn resolve(&self, parent: Vector2) -> Vector2 {
        Vector2::new(self.x.resolve(parent.x), self.y.resolve(parent.y))
    }
}

impl Add for UDim2 {
    type Output = UDim2;
    #[inline]
    fn add(self, rhs: UDim2) -> UDim2 {
        UDim2 {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
        }
    }
}

impl Sub for UDim2 {
    type Output = UDim2;
    #[inline]
    fn sub(self, rhs: UDim2) -> UDim2 {
        UDim2 {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub min: Vector2,
    pub max: Vector2,
}

impl Rect {
    #[inline]
    pub const fn new(min: Vector2, max: Vector2) -> Self {
        Self { min, max }
    }

    #[inline]
    pub const fn from_min_max(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Self {
        Self::new(Vector2::new(min_x, min_y), Vector2::new(max_x, max_y))
    }

    #[inline]
    pub fn width(&self) -> f64 {
        self.max.x - self.min.x
    }

    #[inline]
    pub fn height(&self) -> f64 {
        self.max.y - self.min.y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_mixes_scale_and_offset() {
        let dim = UDim2::new(0.5, 10, 1.0, -4);
        assert_eq!(dim.resolve(Vector2::new(100.0, 100.0)), Vector2::new(60.0, 96.0));
    }

    #[test]
    fn arithmetic_is_componentwise() {
        let a = UDim2::from_offset(1, 2);
        let b = UDim2::from_offset(10, 20);
        assert_eq!(a + b, UDim2::from_offset(11, 22));
        assert_eq!(b - a, UDim2::from_offset(9, 18));
    }

    #[test]
    fn rect_dimensions() {
        let rect = Rect::from_min_max(1.0, 2.0, 5.0, 8.0);
        assert_eq!(rect.width(), 4.0);
        assert_eq!(rect.height(), 6.0);
    }
}
