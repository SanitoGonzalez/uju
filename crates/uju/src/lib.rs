extern crate self as uju;

pub mod ecs;
pub mod math;
pub mod net;
pub mod physics;
pub mod runtime;
pub mod time;
pub mod topology;
pub mod transform;
pub mod util;

#[doc(hidden)]
pub use linkme;

pub mod prelude {
    pub use crate::ecs::prelude::*;
    pub use crate::physics::Velocity;
    pub use crate::time::Time;
    pub use crate::transform::Transform;
}

#[cfg(all(feature = "2d", feature = "3d"))]
compile_error!("features \"2d\" and \"3d\" are mutually exclusive");

#[cfg(not(any(feature = "2d", feature = "3d")))]
compile_error!("one of the features \"2d\", \"3d\" must be enabled");

pub fn init() {
    ecs::init();
}
