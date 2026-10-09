//! `crt::preset::Structure` decides for itself, from a `Config`, what the
//! filter chain's shape is; `config::structural::STRUCTURAL` names the keys
//! whose change forces a rebuild. The relation that has to hold: every
//! config field `Structure::from_config` reads is a structural key, or a
//! change the render layer treats as structural would classify as a live
//! parameter push. The test below holds it by perturbation rather than by
//! a second field list: it moves every serialized leaf of the config and
//! checks that whenever `Structure` moves, the key is structural.

#[test]
fn structure_reads_only_structural_keys() {
    let base = config::Config::default();
    let base_structure = crt::preset::Structure::from_config(&base);
    // Word-enum fields need a valid variant differing from the default;
    // everything else perturbs generically by type.
    let alternates: &[(&str, &str)] = &[
        ("shell", "switchboard"),
        ("channel_indicator", "switch"),
        ("channel_display", "tape"),
        ("rasterization", "scanline_rasterization"),
        ("font_source", "system_fonts"),
        ("selection_model", "rio"),
        ("timing", "random"),
    ];
    let document = serde_json::to_value(&base).expect("config serializes");
    for (section, object) in document.as_object().expect("config is an object") {
        for (key, leaf) in object.as_object().expect("section is an object") {
            let mut moved_doc = document.clone();
            let perturbed = match leaf {
                serde_json::Value::Number(n) if n.is_f64() => {
                    serde_json::Value::from(n.as_f64().unwrap() + 0.5)
                }
                serde_json::Value::Number(n) => serde_json::Value::from(n.as_i64().unwrap() + 1),
                serde_json::Value::Bool(b) => serde_json::Value::from(!b),
                serde_json::Value::String(s) => match alternates.iter().find(|(k, _)| k == key) {
                    Some((_, alternate)) => serde_json::Value::from(*alternate),
                    None => serde_json::Value::from(format!("{s}x")),
                },
                // A list-shaped leaf (`[[ssh.host]]`) has no generic
                // perturbation, and the filter chain reads no list; the
                // day `Structure` reads one, its scalar neighbours still
                // hold the relation this test pins.
                serde_json::Value::Array(_) => continue,
                other => panic!("unexpected leaf shape at {section}.{key}: {other:?}"),
            };
            moved_doc[section][key] = perturbed;
            let moved: config::Config =
                serde_json::from_value(moved_doc).unwrap_or_else(|e| panic!("{section}.{key}: {e}"));
            if crt::preset::Structure::from_config(&moved) != base_structure {
                let dotted = format!("{section}.{key}");
                assert!(
                    config::structural::STRUCTURAL.contains(&dotted.as_str()),
                    "`Structure::from_config` reads `{dotted}`, but it is not a structural key"
                );
            }
        }
    }
}

/// The frame's size and the curvature are computed twice from the same
/// settings, once on each side of a crate boundary neither can cross:
/// `crt::params` bends the picture by them, `app::settings` un-bends a
/// pointer position by them (through `term::distortion`). If they ever
/// disagree, a click lands somewhere other than where the character under
/// the cursor is drawn, and nothing about the picture looks wrong -- which
/// is why this is a test and not a comment.
///
/// The two sides have disagreed before. The render side once had neither
/// the `* 0.05` scale nor the chassis-or-screen split the formula requires,
/// and drew a moulding four and a half times too deep while the pointer
/// inverted the right one. Later the pointer normalised over the well's
/// physical size while the shader normalised over its logical size, which
/// agree only at a scale factor of 1, the scale a restated formula had been
/// checked at. So the scale here is 5/3, and the pointer's side is the
/// app's own derivation rather than a restatement of it.
#[test]
fn both_crates_bend_by_the_same_frame_and_curvature() {
    use config::Config;
    use crt::{DegaussState, Geometry, Params};
    use std::time::{Duration, Instant};
    use term::{CellSize, Viewport};

    // One well, as each side measures it: the pointer in physical pixels,
    // the chain in logical ones. The size is the glass the disagreement
    // first showed on, a 2880-wide panel at 5/3 less its bank; the virtual
    // size and the font scaling feed no uniform checked here.
    let scale = 5.0 / 3.0;
    let viewport = Viewport::new(2462, 1800, scale, CellSize::new(9.0, 18.0));
    let geom = Geometry {
        output_width: (2462.0 / scale) as f32,
        output_height: (1800.0 / scale) as f32,
        virtual_width: 724.0,
        virtual_height: 543.0,
        total_font_scaling: 0.75,
        device_pixel_ratio: scale as f32,
    };

    // Both the shipped default (a chassis stands) and the bare tube, since the
    // key the two sides read is not the same key in the two cases.
    for chassis_shown in [true, false] {
        let mut cfg = Config::default();
        cfg.general.chassis_shown = chassis_shown;
        cfg.chassis.frame_size = 0.45;
        cfg.screen.frame_size = 0.1;
        cfg.screen.screen_curvature = 0.2;

        let mut pacing = crt::Pacing::new(Instant::now());
        let time = pacing.tick_by(Duration::from_millis(16));
        let uniforms = Params::build(&cfg, &geom, time, DegaussState::IDLE);
        let pointer = app::settings::distortion_params(Some(&cfg), &viewport);

        for (name, un_bent) in [
            ("FrameSize", pointer.frame_size),
            (
                "ScreenCurvature",
                pointer.screen_curvature
                    * pointer.screen_curvature_size
                    * pointer.normalized_screen_scale,
            ),
        ] {
            let bent = uniforms
                .get(name)
                .unwrap_or_else(|| panic!("the {name} uniform"));
            assert!(
                (f64::from(bent) - un_bent).abs() < 1e-6,
                "with chassis_shown={chassis_shown} the shader bends {name} by {bent} \
                 and the pointer un-bends by {un_bent}"
            );
        }
    }
}
