//! Inverse screen-curvature distortion.
//!
//! One copy of this exists, in [`term::distortion`], because two features
//! needed it: the pointer path, which turns a click into a cell, and
//! selection, which reasons in grid coordinates. `crates/core`
//! cannot depend on `crates/app`, so the crate both can reach is the home,
//! and this is the name the input path already calls it by.

pub use term::distortion::{correct_distortion, DistortionParams, Point};
