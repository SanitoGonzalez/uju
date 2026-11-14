use std::f32::consts::PI;

use derive_more::From;

#[cfg(feature = "2d")]
pub use parry2d::glamx::Vec2 as Vector;
#[cfg(feature = "3d")]
pub use parry3d::glamx::Vec3 as Vector;

/// A rotation: an angle in 2D, a quaternion in 3D
#[cfg(feature = "2d")]
pub type Rotation = Radians;
#[cfg(feature = "3d")]
pub use parry3d::glamx::Quat as Rotation;

/// A radian angle
#[derive(Clone, Copy, Debug, From, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Radians(f32);

impl From<Degrees> for Radians {
    fn from(degrees: Degrees) -> Self {
        Self(degrees.0 * PI / 180.0)
    }
}

/// A degree angle
#[derive(Clone, Copy, Debug, From, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Degrees(f32);

impl From<Radians> for Degrees {
    fn from(radians: Radians) -> Self {
        Self(radians.0 * 180.0 / PI)
    }
}

/// A unit-per-second scalar
#[derive(Clone, Copy, Debug, From, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Speed(f32);
