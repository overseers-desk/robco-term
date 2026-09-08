//! The parameter/structural key split: which config keys force a
//! filter-chain rebuild (and, by accepted default, a burn-in ghost reset)
//! rather than a live uniform push.
//!
//! [`STRUCTURAL`] is the one encoding of the set; [`classify`] compares two
//! configs at exactly those keys over the schema's own serialization, so
//! there is no second, field-by-field statement of the same knowledge.
//!
//! Rationale for membership, matching the three structural triggers (pass
//! count, framebuffer format, scale):
//!
//! - **Scale / geometry that resizes a framebuffer or glyph atlas**:
//!   `general.window_scaling`, `general.font_scaling`, `screen.font_width`,
//!   `screen.line_spacing`, `screen.margin`, `screen.prose_scaling`,
//!   `screen.frame_size`, `chassis.frame_size`.
//! - **Framebuffer format/resolution** (the bloom and burn-in passes each
//!   own an offscreen framebuffer whose resolution these quality knobs
//!   set): `general.bloom_quality`, `general.burn_in_quality`.
//! - **Pass topology / what is drawn at all** (adds or removes a pass or a
//!   whole chassis draw): `general.chassis_shown`, `chassis.shell`.
//! - **Texture source swap** (a different glyph or LED-bank atlas must be
//!   built and bound, not just re-parameterized): `screen.font_name`,
//!   `screen.font_source`, `chassis.bank_font_name`,
//!   `general.led_characters`, `chassis.channel_indicator`,
//!   `chassis.channel_display`.
//!
//! Explicitly *not* structural, despite being enum/int-coded and once
//! needing a separate compiled shader permutation per value:
//! `screen.rasterization` (raster mode collapses to a runtime int-branch
//! uniform); `screen.burn_in` and `screen.bloom` (continuous
//! decay/intensity uniforms fed every frame, not pass enables -- the
//! *quality* knobs above are what size their framebuffers);
//! `screen.screen_curvature` / `screen.screen_radius` (wired to live
//! uniforms, curvature framed as a runtime parameter);
//! `general.selection_model` (read at the start of each pointer gesture, so
//! a change reaches the next drag without a rebuild and without touching
//! what is on the glass).

use crate::Config;

/// What a config change means for the render pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyClass {
    /// Update the value in place (a uniform push); no rebuild.
    Parameter,
    /// Changing this key must force a filter-chain rebuild (and, by
    /// accepted default, a burn-in ghost reset).
    Structural,
}

/// The structural keys, as dotted paths into the serialized [`Config`].
/// Everything not listed here is `Parameter`.
pub const STRUCTURAL: &[&str] = &[
    "general.window_scaling",
    "general.font_scaling",
    "general.bloom_quality",
    "general.burn_in_quality",
    "general.chassis_shown",
    "general.led_characters",
    "screen.font_name",
    "screen.font_source",
    "screen.font_width",
    "screen.line_spacing",
    "screen.margin",
    "screen.prose_scaling",
    "screen.frame_size",
    "chassis.shell",
    "chassis.channel_indicator",
    "chassis.channel_display",
    "chassis.frame_size",
    "chassis.bank_font_name",
];

/// Compare two config snapshots and say whether the change between them is
/// purely parameter-level (safe to push as uniforms) or touches at least
/// one structural key (needs a filter-chain rebuild).
///
/// Returns [`KeyClass::Parameter`] for `old == new` too: no change is
/// trivially safe to "apply" without a rebuild.
pub fn classify(old: &Config, new: &Config) -> KeyClass {
    let old = serde_json::to_value(old).expect("schema types serialize");
    let new = serde_json::to_value(new).expect("schema types serialize");
    if STRUCTURAL.iter().any(|key| at(&old, key) != at(&new, key)) {
        KeyClass::Structural
    } else {
        KeyClass::Parameter
    }
}

fn at<'v>(value: &'v serde_json::Value, dotted: &str) -> Option<&'v serde_json::Value> {
    dotted
        .split('.')
        .try_fold(value, |value, part| value.get(part))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A structural key is a string, and a string compiles when it is
    /// wrong: every entry must resolve against the schema's serialization,
    /// or deleting a field would leave a straggler here that `classify`
    /// silently stops reading.
    #[test]
    fn every_structural_key_resolves_against_the_schema() {
        let document = serde_json::to_value(Config::default()).expect("schema types serialize");
        for key in STRUCTURAL {
            assert!(
                at(&document, key).is_some(),
                "`{key}` names no field of the serialized Config"
            );
        }
    }

    #[test]
    fn a_structural_move_and_a_parameter_move_classify_apart() {
        let base = Config::default();

        let mut structural = base.clone();
        structural.general.bloom_quality += 0.25;
        assert_eq!(classify(&base, &structural), KeyClass::Structural);

        let mut parameter = base.clone();
        parameter.screen.bloom += 0.25;
        assert_eq!(classify(&base, &parameter), KeyClass::Parameter);

        assert_eq!(classify(&base, &base), KeyClass::Parameter);
    }
}
