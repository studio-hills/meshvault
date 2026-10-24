//! Animated models from schema 1 and 2 exports: data types, transform maths,
//! clip sampling and rig solving. Independent of the UI and renderer.

pub mod budget;
pub mod clip;
pub mod effect;
pub mod math;
pub mod rig;
pub mod schema;

#[cfg(test)]
mod real_export_tests;
