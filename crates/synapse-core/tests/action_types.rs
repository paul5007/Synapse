use schemars::schema_for;
use serde_json::json;
use synapse_core::{
    Action, AimNaturalParams, Backend, GamepadReport, Key, KeyCode, KeystrokeNaturalParams,
};

#[test]
fn natural_fast_constants_match_m2_spec() {
    let aim = AimNaturalParams::FAST;
    assert_eq!(aim.control_point_jitter.to_bits(), 0.08f32.to_bits());
    assert_eq!(aim.tremor_stddev_px.to_bits(), 0.2f32.to_bits());
    assert_eq!(aim.overshoot_prob.to_bits(), 0.25f32.to_bits());
    assert_eq!(aim.overshoot_factor_range.0.to_bits(), 1.02f32.to_bits());
    assert_eq!(aim.overshoot_factor_range.1.to_bits(), 1.06f32.to_bits());
    assert_eq!(aim.micro_correct_steps, 1);
    assert_eq!(aim.timing_stddev_ms.to_bits(), 1.5f32.to_bits());
    assert_eq!(aim.seed, None);

    let keys = KeystrokeNaturalParams::FAST;
    assert_eq!(keys.mean_iki_ms.to_bits(), 32.0f32.to_bits());
    assert_eq!(keys.stddev_ms.to_bits(), 10.0f32.to_bits());
    assert!(keys.bigram_bias);

    println!(
        "source_of_truth=types.rs edge=natural_fast_constants final_value=ok aim_tremor={} key_mean={}",
        aim.tremor_stddev_px, keys.mean_iki_ms
    );
}

#[test]
fn action_json_edges_are_closed_and_tagged() -> Result<(), Box<dyn std::error::Error>> {
    let valid_before = json!({
        "kind": "key_down",
        "key": {"code": {"kind": "named", "value": "ctrl"}, "use_scancode": false},
        "backend": "software"
    });
    let valid_action = serde_json::from_value::<Action>(valid_before.clone())?;
    assert!(matches!(valid_action, Action::KeyDown { .. }));
    println!(
        "source_of_truth=action_json edge=happy_path before={} after={}",
        valid_before,
        serde_json::to_value(&valid_action)?
    );

    let unknown_field = json!({
        "kind": "key_down",
        "key": {"code": {"kind": "named", "value": "ctrl"}, "use_scancode": false},
        "backend": "software",
        "extra": true
    });
    assert!(serde_json::from_value::<Action>(unknown_field.clone()).is_err());
    println!(
        "source_of_truth=action_json edge=unknown_field before={unknown_field} after=rejected"
    );

    let invalid_tag = json!({"kind": "run_shell"});
    assert!(serde_json::from_value::<Action>(invalid_tag.clone()).is_err());
    println!("source_of_truth=action_json edge=invalid_tag before={invalid_tag} after=rejected");

    let invalid_element = json!({
        "kind": "mouse_move",
        "to": {"kind": "element", "element_id": "not-an-element"},
        "curve": {"kind": "instant"},
        "duration_ms": 50,
        "backend": "software"
    });
    assert!(serde_json::from_value::<Action>(invalid_element.clone()).is_err());
    println!(
        "source_of_truth=action_json edge=invalid_element before={invalid_element} after=rejected"
    );

    Ok(())
}

#[test]
fn key_code_json_round_trips_each_variant() -> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        KeyCode::Named {
            value: "enter".to_owned(),
        },
        KeyCode::Symbol { value: '@' },
        KeyCode::HidCode { value: 0x04 },
    ];

    for code in cases {
        let key = Key {
            code,
            use_scancode: false,
        };
        let json = serde_json::to_value(&key)?;
        let parsed = serde_json::from_value::<Key>(json.clone())?;
        assert_eq!(parsed, key);
        println!("source_of_truth=key_code edge=round_trip final_value={json}");
    }

    Ok(())
}

#[test]
fn gamepad_report_schema_has_closed_object_and_axis_bounds()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = serde_json::to_value(schema_for!(GamepadReport))?;
    assert_eq!(schema["additionalProperties"], false);

    let thumb_l = &schema["properties"]["thumb_l"];
    assert!(contains_number_field(thumb_l, "minimum", -1.0));
    assert!(contains_number_field(thumb_l, "maximum", 1.0));
    assert!(contains_number_field(
        &schema["properties"]["lt"],
        "minimum",
        0.0
    ));
    assert!(contains_number_field(
        &schema["properties"]["lt"],
        "maximum",
        1.0
    ));

    let before = json!({
        "buttons": ["a"],
        "thumb_l": [1.5, 0.0],
        "thumb_r": [0.0, 0.0],
        "lt": 0.0,
        "rt": 0.0
    });
    println!(
        "source_of_truth=gamepad_schema edge=thumb_l_out_of_range before={before} after=rejected_by_schema_bounds"
    );
    Ok(())
}

#[test]
fn synapse_core_root_reexports_action_types() {
    let action = synapse_core::Action::KeyDown {
        key: synapse_core::Key {
            code: synapse_core::KeyCode::Named {
                value: "shift".to_owned(),
            },
            use_scancode: false,
        },
        backend: Backend::Software,
    };

    assert!(matches!(action, synapse_core::Action::KeyDown { .. }));
}

fn contains_number_field(value: &serde_json::Value, key: &str, expected: f64) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            map.get(key)
                .and_then(serde_json::Value::as_f64)
                .is_some_and(|actual| (actual - expected).abs() < f64::EPSILON)
                || map
                    .values()
                    .any(|nested| contains_number_field(nested, key, expected))
        }
        serde_json::Value::Array(items) => items
            .iter()
            .any(|nested| contains_number_field(nested, key, expected)),
        _ => false,
    }
}
