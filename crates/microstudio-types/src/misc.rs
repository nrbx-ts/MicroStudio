use serde::{Deserialize, Serialize};

use crate::{Color3, Vector3};

// direction need not be unit
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Ray {
    pub origin: Vector3,
    pub direction: Vector3,
}

impl Ray {
    #[inline]
    pub const fn new(origin: Vector3, direction: Vector3) -> Self {
        Self { origin, direction }
    }

    #[inline]
    pub fn unit_direction(&self) -> Vector3 {
        self.direction.unit()
    }

    // on the ray's line, not the segment
    #[inline]
    pub fn closest_point_t(&self, point: Vector3) -> f64 {
        let len_sq = self.direction.magnitude_squared();
        if len_sq == 0.0 {
            return 0.0;
        }
        (point - self.origin).dot(&self.direction) / len_sq
    }

    #[inline]
    pub fn closest_point(&self, point: Vector3) -> Vector3 {
        self.origin + self.direction * self.closest_point_t(point)
    }

    #[inline]
    pub fn distance(&self, point: Vector3) -> f64 {
        (point - self.closest_point(point)).magnitude()
    }

    #[inline]
    pub fn at(&self, t: f64) -> Vector3 {
        self.origin + self.direction * t
    }
}

#[derive(Debug, Clone, Copy)]
struct PaletteEntry {
    number: u16,
    name: &'static str,
    rgb: (u8, u8, u8),
}

// hand-checked subset; unknown numbers fall back to grey
const PALETTE: &[PaletteEntry] = &[
    PaletteEntry { number: 1, name: "White", rgb: (242, 243, 243) },
    PaletteEntry { number: 2, name: "Grey", rgb: (161, 165, 162) },
    PaletteEntry { number: 3, name: "Light yellow", rgb: (249, 233, 153) },
    PaletteEntry { number: 5, name: "Brick yellow", rgb: (215, 197, 154) },
    PaletteEntry { number: 6, name: "Light green (Mint)", rgb: (194, 218, 184) },
    PaletteEntry { number: 9, name: "Light reddish violet", rgb: (232, 186, 200) },
    PaletteEntry { number: 11, name: "Pastel Blue", rgb: (128, 187, 219) },
    PaletteEntry { number: 12, name: "Light orange brown", rgb: (203, 132, 66) },
    PaletteEntry { number: 18, name: "Nougat", rgb: (204, 142, 105) },
    PaletteEntry { number: 21, name: "Bright red", rgb: (196, 40, 28) },
    PaletteEntry { number: 22, name: "Med. reddish violet", rgb: (196, 112, 160) },
    PaletteEntry { number: 23, name: "Bright blue", rgb: (13, 105, 172) },
    PaletteEntry { number: 24, name: "Bright yellow", rgb: (245, 205, 48) },
    PaletteEntry { number: 25, name: "Earth orange", rgb: (98, 71, 50) },
    PaletteEntry { number: 26, name: "Black", rgb: (27, 42, 53) },
    PaletteEntry { number: 27, name: "Dark grey", rgb: (109, 110, 108) },
    PaletteEntry { number: 28, name: "Dark green", rgb: (40, 127, 71) },
    PaletteEntry { number: 37, name: "Bright green", rgb: (75, 151, 75) },
    PaletteEntry { number: 100, name: "Light red", rgb: (238, 196, 182) },
    PaletteEntry { number: 101, name: "Medium red", rgb: (218, 134, 122) },
    PaletteEntry { number: 102, name: "Salmon", rgb: (219, 145, 145) },
    PaletteEntry { number: 104, name: "Bright violet", rgb: (107, 50, 124) },
    PaletteEntry { number: 105, name: "Br. yellowish orange", rgb: (226, 155, 64) },
    PaletteEntry { number: 106, name: "Bright orange", rgb: (218, 133, 65) },
    PaletteEntry { number: 107, name: "Bright bluish green", rgb: (0, 143, 156) },
    PaletteEntry { number: 119, name: "Br. yellowish green", rgb: (164, 189, 71) },
    PaletteEntry { number: 141, name: "Earth green", rgb: (39, 70, 45) },
    PaletteEntry { number: 194, name: "Medium stone grey", rgb: (163, 162, 165) },
    PaletteEntry { number: 199, name: "Dark stone grey", rgb: (99, 95, 98) },
    PaletteEntry { number: 208, name: "Light stone grey", rgb: (229, 228, 223) },
    PaletteEntry { number: 217, name: "Brown", rgb: (124, 92, 70) },
    PaletteEntry { number: 1001, name: "Institutional white", rgb: (248, 248, 248) },
    PaletteEntry { number: 1002, name: "Mid gray", rgb: (205, 205, 205) },
    PaletteEntry { number: 1003, name: "Really black", rgb: (17, 17, 17) },
    PaletteEntry { number: 1004, name: "Really red", rgb: (255, 0, 0) },
    PaletteEntry { number: 1006, name: "Really blue", rgb: (0, 0, 255) },
    PaletteEntry { number: 1009, name: "Really lime green", rgb: (0, 255, 0) },
    PaletteEntry { number: 1014, name: "Bright red (Legacy)", rgb: (255, 0, 0) },
];

const FALLBACK_RGB: (u8, u8, u8) = (128, 128, 128);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BrickColor {
    number: u16,
}

impl BrickColor {
    #[inline]
    pub const fn from_number(number: u16) -> Self {
        Self { number }
    }

    #[inline]
    pub const fn number(&self) -> u16 {
        self.number
    }

    pub fn name(&self) -> &'static str {
        lookup(self.number).map(|e| e.name).unwrap_or("Unknown")
    }

    pub fn color(&self) -> Color3 {
        let (r, g, b) = lookup(self.number).map(|e| e.rgb).unwrap_or(FALLBACK_RGB);
        Color3::from_rgb(r, g, b)
    }

    pub fn r(&self) -> f64 {
        self.color().r
    }

    pub fn g(&self) -> f64 {
        self.color().g
    }

    pub fn b(&self) -> f64 {
        self.color().b
    }

    pub fn from_name(name: &str) -> Option<Self> {
        PALETTE
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(name))
            .map(|e| Self::from_number(e.number))
    }

    pub fn from_color3(color: Color3) -> Self {
        let mut best: Option<(f64, u16)> = None;
        for entry in PALETTE {
            let (r, g, b) = entry.rgb;
            let candidate = Color3::from_rgb(r, g, b);
            let dr = color.r - candidate.r;
            let dg = color.g - candidate.g;
            let db = color.b - candidate.b;
            let distance = dr * dr + dg * dg + db * db;
            if best.map_or(true, |(bd, _)| distance < bd) {
                best = Some((distance, entry.number));
            }
        }
        Self::from_number(best.map(|(_, n)| n).unwrap_or(0))
    }
}

fn lookup(number: u16) -> Option<&'static PaletteEntry> {
    PALETTE.iter().find(|e| e.number == number)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_distance_and_closest_point() {
        let ray = Ray::new(Vector3::zero(), Vector3::X_AXIS);
        assert_eq!(ray.distance(Vector3::new(3.0, 4.0, 0.0)), 4.0);
        assert!(ray.closest_point(Vector3::new(3.0, 4.0, 0.0)).fuzzy_eq(&Vector3::new(3.0, 0.0, 0.0)));
    }

    #[test]
    fn ray_handles_near_zero_direction() {
        let ray = Ray::new(Vector3::new(1.0, 1.0, 1.0), Vector3::zero());
        assert_eq!(ray.closest_point_t(Vector3::new(5.0, 5.0, 5.0)), 0.0);
    }

    #[test]
    fn brick_color_lookup() {
        let black = BrickColor::from_number(26);
        assert_eq!(black.name(), "Black");
        assert!(black.color().fuzzy_eq(&Color3::from_rgb(27, 42, 53)));

        assert_eq!(BrickColor::from_name("Bright red").unwrap().number(), 21);
        assert_eq!(BrickColor::from_number(9999).name(), "Unknown");
    }

    #[test]
    fn brick_color_nearest_match() {
        assert_eq!(BrickColor::from_color3(Color3::from_rgb(255, 0, 0)).number(), 1004);
    }
}
