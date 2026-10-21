//! Animated models from schema v1 exports: data types, transform maths,
//! clip sampling and rig solving. Independent of the UI and renderer.

pub mod clip;
pub mod math;
pub mod rig;
pub mod schema;

#[cfg(test)]
mod real_export_tests;
