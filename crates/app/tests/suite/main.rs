//! The crate's integration tests, one module per concern, compiled as a
//! single test binary: each separate integration-test file links its own
//! ~200MB copy of the dependency stack, and this workspace's suite once
//! weighed 21GB that way.

mod bank_chrome;
mod bank_furniture;
mod channel_bank;
mod clipboard_keys;
mod critters;
mod find_keys;
mod frame_stats;
mod fullscreen_pointer;
mod ime;
mod key_bindings;
mod keyboard_scroll;
mod meta_keys;
mod one_instance;
mod pointer;
mod pointer_live_settings;
mod profile_cli;
mod profile_pixels;
mod redraw_pacing;
mod seam_drag;
mod selection_drift;
mod settings_live_reload;
mod shed_notice;
mod size_badge;
// The far side's agent serves on a Unix socket; until a named-pipe test
// agent exists, the SSH flow suite compiles on Unix alone.
#[cfg(unix)]
mod ssh_flow;
mod structure_subset;
mod tmux_flow;
