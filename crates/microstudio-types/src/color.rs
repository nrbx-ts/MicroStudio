use serde::{Deserialize, Serialize};
use std::ops::{Add, Mul, Sub};

use crate::fuzzy_eq;

// components in [0, 1]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Color3 {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Color3Hsv {
    pub h: f64,
    pub s: f64,
    pub v: f64,
}

impl Color3 {
    #[inline]
    pub const fn new(r: f64, g: f64, b: f64) -> Self {
        Self { r, g, b }
    }

    // 0..=255 channels
    #[inline]
    pub fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Self::new(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0)
    }

    // accepts #rrggbb or rrggbb
    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.trim().trim_start_matches('#');
        if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let parse = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        Some(Self::from_rgb(parse(0)?, parse(2)?, parse(4)?))
    }

    pub fn from_hsv(h: f64, s: f64, v: f64) -> Self {
        let h = h.rem_euclid(1.0);
        let s = s.clamp(0.0, 1.0);
        let v = v.clamp(0.0, 1.0);
        if s == 0.0 {
            return Self::new(v, v, v);
        }
        let sector = h * 6.0;
        let i = sector.floor() as i32;
        let f = sector - i as f64;
        let p = v * (1.0 - s);
        let q = v * (1.0 - s * f);
        let t = v * (1.0 - s * (1.0 - f));
        match i.rem_euclid(6) {
            0 => Self::new(v, t, p),
            1 => Self::new(q, v, p),
            2 => Self::new(p, v, t),
            3 => Self::new(p, q, v),
            4 => Self::new(t, p, v),
            _ => Self::new(v, p, q),
        }
    }

    pub fn to_hsv(&self) -> Color3Hsv {
        let max = self.r.max(self.g).max(self.b);
        let min = self.r.min(self.g).min(self.b);
        let delta = max - min;

        let h = if delta == 0.0 {
            0.0
        } else if max == self.r {
            ((self.g - self.b) / delta).rem_euclid(6.0) / 6.0
        } else if max == self.g {
            (((self.b - self.r) / delta) + 2.0) / 6.0
        } else {
            (((self.r - self.g) / delta) + 4.0) / 6.0
        };
        let s = if max == 0.0 { 0.0 } else { delta / max };

        Color3Hsv { h, s, v: max }
    }

    pub fn to_hex(&self) -> String {
        let channel = |c: f64| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02X}{:02X}{:02X}", channel(self.r), channel(self.g), channel(self.b))
    }

    #[inline]
    pub fn lerp(&self, other: &Color3, alpha: f64) -> Color3 {
        Color3::new(
            self.r + (other.r - self.r) * alpha,
            self.g + (other.g - self.g) * alpha,
            self.b + (other.b - self.b) * alpha,
        )
    }

    #[inline]
    pub fn fuzzy_eq(&self, other: &Color3) -> bool {
        fuzzy_eq(self.r, other.r) && fuzzy_eq(self.g, other.g) && fuzzy_eq(self.b, other.b)
    }
}

impl Add for Color3 {
    type Output = Color3;
    #[inline]
    fn add(self, rhs: Color3) -> Color3 {
        Color3::new(self.r + rhs.r, self.g + rhs.g, self.b + rhs.b)
    }
}

impl Sub for Color3 {
    type Output = Color3;
    #[inline]
    fn sub(self, rhs: Color3) -> Color3 {
        Color3::new(self.r - rhs.r, self.g - rhs.g, self.b - rhs.b)
    }
}

impl Mul<Color3> for Color3 {
    type Output = Color3;
    #[inline]
    fn mul(self, rhs: Color3) -> Color3 {
        Color3::new(self.r * rhs.r, self.g * rhs.g, self.b * rhs.b)
    }
}

impl Mul<f64> for Color3 {
    type Output = Color3;
    #[inline]
    fn mul(self, rhs: f64) -> Color3 {
        Color3::new(self.r * rhs, self.g * rhs, self.b * rhs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_rgb_scales_to_unit_range() {
        let c = Color3::from_rgb(255, 0, 128);
        assert!(fuzzy_eq(c.r, 1.0));
        assert!(fuzzy_eq(c.g, 0.0));
        assert!(fuzzy_eq(c.b, 128.0 / 255.0));
    }

    #[test]
    fn hex_round_trips() {
        let c = Color3::from_hex("#FF0080").unwrap();
        assert_eq!(c.to_hex(), "#FF0080");
        assert_eq!(Color3::from_hex("ff0080").unwrap(), c);
        assert!(Color3::from_hex("nope").is_none());
    }

    #[test]
    fn hsv_round_trips_primaries() {
        let red = Color3::new(1.0, 0.0, 0.0);
        let hsv = red.to_hsv();
        assert!(fuzzy_eq(hsv.h, 0.0));
        assert!(fuzzy_eq(hsv.s, 1.0));
        assert!(fuzzy_eq(hsv.v, 1.0));
        assert!(Color3::from_hsv(hsv.h, hsv.s, hsv.v).fuzzy_eq(&red));

        let green = Color3::new(0.0, 1.0, 0.0);
        assert!(Color3::from_hsv(green.to_hsv().h, 1.0, 1.0).fuzzy_eq(&green));

        let blue = Color3::new(0.0, 0.0, 1.0);
        assert!(Color3::from_hsv(blue.to_hsv().h, 1.0, 1.0).fuzzy_eq(&blue));
    }

    #[test]
    fn lerp_midpoint() {
        let a = Color3::new(0.0, 0.5, 1.0);
        let b = Color3::new(1.0, 0.5, 0.0);
        assert!(a.lerp(&b, 0.5).fuzzy_eq(&Color3::new(0.5, 0.5, 0.5)));
    }
}
