use serde::{Deserialize, Serialize};
use std::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};

use crate::fuzzy_eq;

macro_rules! vector_type {
    ($name:ident, $($field:ident),+) => {
        #[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
        pub struct $name {
            $(pub $field: f64,)+
        }

        impl $name {
            #[inline]
            pub const fn new($($field: f64),+) -> Self {
                Self { $($field,)+ }
            }

            #[inline]
            pub fn magnitude(&self) -> f64 {
                self.magnitude_squared().sqrt()
            }

            #[inline]
            pub fn magnitude_squared(&self) -> f64 {
                0.0 $(+ self.$field * self.$field)+
            }

            #[inline]
            pub fn unit(&self) -> Self {
                let m = self.magnitude();
                if m == 0.0 { Self::zero() } else { *self / m }
            }

            #[inline]
            pub fn dot(&self, other: &Self) -> f64 {
                0.0 $(+ self.$field * other.$field)+
            }

            #[inline]
            pub fn lerp(&self, other: &Self, alpha: f64) -> Self {
                *self + (*other - *self) * alpha
            }

            #[inline]
            pub fn fuzzy_eq(&self, other: &Self) -> bool {
                true $(&& fuzzy_eq(self.$field, other.$field))+
            }

            #[inline]
            pub fn is_zero(&self) -> bool {
                true $(&& self.$field == 0.0)+
            }

            #[inline]
            pub fn component(&self, index: usize) -> f64 {
                let components = [$(self.$field),+];
                components.get(index).copied().unwrap_or(0.0)
            }
        }

        impl Add for $name {
            type Output = Self;
            #[inline]
            fn add(self, rhs: Self) -> Self {
                Self::new($(self.$field + rhs.$field),+)
            }
        }

        impl Sub for $name {
            type Output = Self;
            #[inline]
            fn sub(self, rhs: Self) -> Self {
                Self::new($(self.$field - rhs.$field),+)
            }
        }

        impl Mul<f64> for $name {
            type Output = Self;
            #[inline]
            fn mul(self, rhs: f64) -> Self {
                Self::new($(self.$field * rhs),+)
            }
        }

        impl Div<f64> for $name {
            type Output = Self;
            #[inline]
            fn div(self, rhs: f64) -> Self {
                Self::new($(self.$field / rhs),+)
            }
        }

        impl Neg for $name {
            type Output = Self;
            #[inline]
            fn neg(self) -> Self {
                Self::new($(-self.$field),+)
            }
        }

        impl AddAssign for $name {
            #[inline]
            fn add_assign(&mut self, rhs: Self) {
                *self = *self + rhs;
            }
        }

        impl SubAssign for $name {
            #[inline]
            fn sub_assign(&mut self, rhs: Self) {
                *self = *self - rhs;
            }
        }

        impl MulAssign<f64> for $name {
            #[inline]
            fn mul_assign(&mut self, rhs: f64) {
                *self = *self * rhs;
            }
        }

        impl DivAssign<f64> for $name {
            #[inline]
            fn div_assign(&mut self, rhs: f64) {
                *self = *self / rhs;
            }
        }
    };
}

vector_type!(Vector2, x, y);
vector_type!(Vector3, x, y, z);

impl Vector2 {
    #[inline]
    pub const fn zero() -> Self {
        Self::new(0.0, 0.0)
    }

    #[inline]
    pub const fn one() -> Self {
        Self::new(1.0, 1.0)
    }
}

impl Vector3 {
    #[inline]
    pub const fn zero() -> Self {
        Self::new(0.0, 0.0, 0.0)
    }

    #[inline]
    pub const fn one() -> Self {
        Self::new(1.0, 1.0, 1.0)
    }

    pub const X_AXIS: Vector3 = Vector3::new(1.0, 0.0, 0.0);
    pub const Y_AXIS: Vector3 = Vector3::new(0.0, 1.0, 0.0);
    pub const Z_AXIS: Vector3 = Vector3::new(0.0, 0.0, 1.0);

    // luau Vector3 * Vector3
    #[inline]
    pub fn mul_componentwise(self, rhs: Self) -> Self {
        Self::new(self.x * rhs.x, self.y * rhs.y, self.z * rhs.z)
    }

    #[inline]
    pub fn cross(self, rhs: Self) -> Self {
        Self::new(
            self.y * rhs.z - self.z * rhs.y,
            self.z * rhs.x - self.x * rhs.z,
            self.x * rhs.y - self.y * rhs.x,
        )
    }

    #[inline]
    pub fn min(self, rhs: Self) -> Self {
        Self::new(self.x.min(rhs.x), self.y.min(rhs.y), self.z.min(rhs.z))
    }

    #[inline]
    pub fn max(self, rhs: Self) -> Self {
        Self::new(self.x.max(rhs.x), self.y.max(rhs.y), self.z.max(rhs.z))
    }

    #[inline]
    pub fn abs(self) -> Self {
        Self::new(self.x.abs(), self.y.abs(), self.z.abs())
    }

    #[inline]
    pub fn floor(self) -> Self {
        Self::new(self.x.floor(), self.y.floor(), self.z.floor())
    }

    #[inline]
    pub fn ceil(self) -> Self {
        Self::new(self.x.ceil(), self.y.ceil(), self.z.ceil())
    }

    #[inline]
    pub fn sign(self) -> Self {
        Self::new(self.x.signum(), self.y.signum(), self.z.signum())
    }

    #[inline]
    pub fn angle(self, rhs: Self) -> f64 {
        let denom = self.magnitude() * rhs.magnitude();
        if denom == 0.0 {
            return 0.0;
        }
        (self.dot(&rhs) / denom).clamp(-1.0, 1.0).acos()
    }

    // radians
    #[inline]
    pub fn rotate(self, axis: Vector3, angle: f64) -> Self {
        let axis = axis.unit();
        let (sin, cos) = angle.sin_cos();
        self * cos + axis.cross(self) * sin + axis * (axis.dot(&self) * (1.0 - cos))
    }
}

impl Vector2 {
    // luau Vector2 * Vector2
    #[inline]
    pub fn mul_componentwise(self, rhs: Self) -> Self {
        Self::new(self.x * rhs.x, self.y * rhs.y)
    }

    #[inline]
    pub fn cross(self, rhs: Self) -> f64 {
        self.x * rhs.y - self.y * rhs.x
    }

    #[inline]
    pub fn min(self, rhs: Self) -> Self {
        Self::new(self.x.min(rhs.x), self.y.min(rhs.y))
    }

    #[inline]
    pub fn max(self, rhs: Self) -> Self {
        Self::new(self.x.max(rhs.x), self.y.max(rhs.y))
    }

    #[inline]
    pub fn abs(self) -> Self {
        Self::new(self.x.abs(), self.y.abs())
    }
}

impl From<Vector2> for Vector3 {
    #[inline]
    fn from(value: Vector2) -> Self {
        Vector3::new(value.x, value.y, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic() {
        let a = Vector3::new(1.0, 2.0, 3.0);
        let b = Vector3::new(4.0, 5.0, 6.0);
        assert_eq!(a + b, Vector3::new(5.0, 7.0, 9.0));
        assert_eq!(b - a, Vector3::new(3.0, 3.0, 3.0));
        assert_eq!(a * 2.0, Vector3::new(2.0, 4.0, 6.0));
        assert_eq!(-a, Vector3::new(-1.0, -2.0, -3.0));
    }

    #[test]
    fn products() {
        let a = Vector3::new(1.0, 0.0, 0.0);
        let b = Vector3::new(0.0, 1.0, 0.0);
        assert_eq!(a.dot(&b), 0.0);
        assert_eq!(a.cross(b), Vector3::new(0.0, 0.0, 1.0));
        assert_eq!(Vector3::new(3.0, 4.0, 0.0).magnitude(), 5.0);
        assert!(Vector3::new(3.0, 4.0, 0.0).unit().fuzzy_eq(&Vector3::new(0.6, 0.8, 0.0)));
    }

    #[test]
    fn lerp_and_components() {
        let a = Vector3::zero();
        let b = Vector3::new(10.0, 20.0, 30.0);
        assert_eq!(a.lerp(&b, 0.5), Vector3::new(5.0, 10.0, 15.0));
        assert_eq!(b.component(0), 10.0);
        assert_eq!(b.component(2), 30.0);
        assert_eq!(b.component(9), 0.0);
    }

    #[test]
    fn vector2_basics() {
        let a = Vector2::new(1.0, 2.0);
        assert_eq!(a.magnitude_squared(), 5.0);
        assert_eq!(a.cross(Vector2::new(3.0, 4.0)), -2.0);
    }
}
