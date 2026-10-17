//! The crate's integration tests, one module per concern, compiled as a
//! single test binary: each separate integration-test file links its own
//! ~200MB copy of the dependency stack, and this workspace's suite once
//! weighed 21GB that way.

mod antialias;
mod critter_paint;
mod fallback;
mod font_parity;
mod grid_tests;
mod hotspot_tests;
mod link_tests;
mod pixel_properties;
mod pointer_tests;
mod preedit;
mod rio_grid_tests;
mod scrollback;
mod search_tests;
mod selection_konsole_tests;
mod selection_paint;
mod selection_rio_tests;
mod system_fonts;
mod transcript;
