extern crate self as uju;

pub mod ecs;
pub mod math;
pub mod mesh;
pub mod net;
pub mod util;
pub mod wire;

#[doc(hidden)]
pub use linkme;

#[cfg(all(feature = "2d", feature = "3d"))]
compile_error!("features \"2d\" and \"3d\" are mutually exclusive");

#[cfg(not(any(feature = "2d", feature = "3d")))]
compile_error!("one of the features \"2d\", \"3d\" must be enabled");

pub fn init() {
    ecs::init();
}
