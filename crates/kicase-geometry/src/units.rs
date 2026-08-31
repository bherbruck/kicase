//! Strongly typed physical quantities.
//!
//! Every public geometry API in KiCase speaks [`Length`] rather than a naked
//! `f64`. Millimetres are the external unit; conversion from KiCad's internal
//! nanometres happens once, at the `kicase-kicad` boundary.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

/// A linear distance, stored in millimetres.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct Length(f64);

impl Length {
    pub const ZERO: Length = Length(0.0);

    /// Number of KiCad internal units (nanometres) per millimetre.
    pub const NM_PER_MM: f64 = 1_000_000.0;

    #[inline]
    pub const fn from_mm(mm: f64) -> Self {
        Length(mm)
    }

    /// Converts from KiCad's internal representation (nanometres).
    #[inline]
    pub fn from_nm(nm: i64) -> Self {
        Length(nm as f64 / Self::NM_PER_MM)
    }

    #[inline]
    pub const fn mm(self) -> f64 {
        self.0
    }

    #[inline]
    pub fn nm(self) -> i64 {
        (self.0 * Self::NM_PER_MM).round() as i64
    }

    #[inline]
    pub fn abs(self) -> Self {
        Length(self.0.abs())
    }

    #[inline]
    pub fn is_positive(self) -> bool {
        self.0 > 0.0
    }

    #[inline]
    pub fn is_finite(self) -> bool {
        self.0.is_finite()
    }

    #[inline]
    pub fn min(self, other: Self) -> Self {
        Length(self.0.min(other.0))
    }

    #[inline]
    pub fn max(self, other: Self) -> Self {
        Length(self.0.max(other.0))
    }

    /// Compares two lengths within `tol`.
    #[inline]
    pub fn approx_eq(self, other: Self, tol: Length) -> bool {
        (self.0 - other.0).abs() <= tol.0
    }
}

impl fmt::Display for Length {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.3} mm", self.0)
    }
}

impl Add for Length {
    type Output = Length;
    #[inline]
    fn add(self, rhs: Length) -> Length {
        Length(self.0 + rhs.0)
    }
}

impl AddAssign for Length {
    #[inline]
    fn add_assign(&mut self, rhs: Length) {
        self.0 += rhs.0;
    }
}

impl Sub for Length {
    type Output = Length;
    #[inline]
    fn sub(self, rhs: Length) -> Length {
        Length(self.0 - rhs.0)
    }
}

impl SubAssign for Length {
    #[inline]
    fn sub_assign(&mut self, rhs: Length) {
        self.0 -= rhs.0;
    }
}

impl Neg for Length {
    type Output = Length;
    #[inline]
    fn neg(self) -> Length {
        Length(-self.0)
    }
}

impl Mul<f64> for Length {
    type Output = Length;
    #[inline]
    fn mul(self, rhs: f64) -> Length {
        Length(self.0 * rhs)
    }
}

impl Mul<Length> for f64 {
    type Output = Length;
    #[inline]
    fn mul(self, rhs: Length) -> Length {
        Length(self * rhs.0)
    }
}

impl Div<f64> for Length {
    type Output = Length;
    #[inline]
    fn div(self, rhs: f64) -> Length {
        Length(self.0 / rhs)
    }
}

/// Ratio of two lengths is dimensionless.
impl Div<Length> for Length {
    type Output = f64;
    #[inline]
    fn div(self, rhs: Length) -> f64 {
        self.0 / rhs.0
    }
}

impl std::iter::Sum for Length {
    fn sum<I: Iterator<Item = Length>>(iter: I) -> Length {
        Length(iter.map(|l| l.0).sum())
    }
}

/// Convenience constructor: `mm(2.0)`.
#[inline]
pub const fn mm(value: f64) -> Length {
    Length::from_mm(value)
}

/// An angle, stored in radians.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct Angle(f64);

impl Angle {
    pub const ZERO: Angle = Angle(0.0);

    #[inline]
    pub const fn from_radians(radians: f64) -> Self {
        Angle(radians)
    }

    #[inline]
    pub fn from_degrees(degrees: f64) -> Self {
        Angle(degrees.to_radians())
    }

    #[inline]
    pub const fn radians(self) -> f64 {
        self.0
    }

    #[inline]
    pub fn degrees(self) -> f64 {
        self.0.to_degrees()
    }

    /// Normalizes into `(-PI, PI]`.
    pub fn normalized(self) -> Self {
        let tau = std::f64::consts::TAU;
        let mut a = self.0 % tau;
        if a > std::f64::consts::PI {
            a -= tau;
        } else if a <= -std::f64::consts::PI {
            a += tau;
        }
        Angle(a)
    }
}

impl Add for Angle {
    type Output = Angle;
    #[inline]
    fn add(self, rhs: Angle) -> Angle {
        Angle(self.0 + rhs.0)
    }
}

impl Sub for Angle {
    type Output = Angle;
    #[inline]
    fn sub(self, rhs: Angle) -> Angle {
        Angle(self.0 - rhs.0)
    }
}

impl Neg for Angle {
    type Output = Angle;
    #[inline]
    fn neg(self) -> Angle {
        Angle(-self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nm_round_trip() {
        assert_eq!(Length::from_nm(1_500_000).mm(), 1.5);
        assert_eq!(mm(2.5).nm(), 2_500_000);
    }

    #[test]
    fn arithmetic() {
        assert_eq!(mm(2.0) + mm(3.0), mm(5.0));
        assert_eq!(mm(5.0) - mm(3.0), mm(2.0));
        assert_eq!(mm(2.0) * 3.0, mm(6.0));
        assert_eq!(mm(6.0) / 2.0, mm(3.0));
        assert_eq!(mm(6.0) / mm(2.0), 3.0);
    }

    #[test]
    fn angle_normalization() {
        let a = Angle::from_degrees(370.0).normalized();
        assert!((a.degrees() - 10.0).abs() < 1e-9);
    }
}
