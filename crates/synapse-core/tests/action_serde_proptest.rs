use proptest::{
    collection::vec,
    prelude::*,
    test_runner::{Config, TestRng, TestRunner},
};
use synapse_core::{
    Action, AimCurve, AimNaturalParams, AimStyle, AimTarget, Backend, ButtonAction, ComboInput,
    ComboStep, ElementId, GamepadReport, Key, KeyCode, KeystrokeDynamics, KeystrokeNaturalParams,
    MouseButton, MouseTarget, PadButton, Point, Stick, Trigger,
};

#[test]
fn action_key_press_round_trips_1000_cases() -> Result<(), Box<dyn std::error::Error>> {
    run_action_round_trip(
        "key_press",
        (key_strategy(), 0u32..=30_000, backend_strategy()).prop_map(|(key, hold_ms, backend)| {
            Action::KeyPress {
                key,
                hold_ms,
                backend,
            }
        }),
    )
}

#[test]
fn action_key_down_round_trips_1000_cases() -> Result<(), Box<dyn std::error::Error>> {
    run_action_round_trip(
        "key_down",
        (key_strategy(), backend_strategy())
            .prop_map(|(key, backend)| Action::KeyDown { key, backend }),
    )
}

#[test]
fn action_key_up_round_trips_1000_cases() -> Result<(), Box<dyn std::error::Error>> {
    run_action_round_trip(
        "key_up",
        (key_strategy(), backend_strategy())
            .prop_map(|(key, backend)| Action::KeyUp { key, backend }),
    )
}

#[test]
fn action_key_chord_round_trips_1000_cases() -> Result<(), Box<dyn std::error::Error>> {
    run_action_round_trip(
        "key_chord",
        (
            vec(key_strategy(), 0..=4),
            0u32..=30_000,
            backend_strategy(),
        )
            .prop_map(|(keys, hold_ms, backend)| Action::KeyChord {
                keys,
                hold_ms,
                backend,
            }),
    )
}

#[test]
fn action_type_text_round_trips_1000_cases() -> Result<(), Box<dyn std::error::Error>> {
    run_action_round_trip(
        "type_text",
        (text_strategy(), dynamics_strategy(), backend_strategy()).prop_map(
            |(text, dynamics, backend)| Action::TypeText {
                text,
                dynamics,
                backend,
            },
        ),
    )
}

#[test]
fn action_mouse_move_round_trips_1000_cases() -> Result<(), Box<dyn std::error::Error>> {
    run_action_round_trip(
        "mouse_move",
        (
            mouse_target_strategy(),
            aim_curve_strategy(),
            0u32..=1_000,
            backend_strategy(),
        )
            .prop_map(|(to, curve, duration_ms, backend)| Action::MouseMove {
                to,
                curve,
                duration_ms,
                backend,
            }),
    )
}

#[test]
fn action_mouse_move_relative_round_trips_1000_cases() -> Result<(), Box<dyn std::error::Error>> {
    run_action_round_trip(
        "mouse_move_relative",
        (coord_strategy(), coord_strategy(), backend_strategy())
            .prop_map(|(dx, dy, backend)| Action::MouseMoveRelative { dx, dy, backend }),
    )
}

#[test]
fn action_mouse_button_round_trips_1000_cases() -> Result<(), Box<dyn std::error::Error>> {
    run_action_round_trip(
        "mouse_button",
        (
            mouse_button_strategy(),
            button_action_strategy(),
            0u32..=30_000,
            backend_strategy(),
        )
            .prop_map(|(button, action, hold_ms, backend)| Action::MouseButton {
                button,
                action,
                hold_ms,
                backend,
            }),
    )
}

#[test]
fn action_mouse_drag_round_trips_1000_cases() -> Result<(), Box<dyn std::error::Error>> {
    run_action_round_trip(
        "mouse_drag",
        (
            point_strategy(),
            point_strategy(),
            mouse_button_strategy(),
            aim_curve_strategy(),
            0u32..=1_000,
            backend_strategy(),
        )
            .prop_map(|(from, to, button, curve, duration_ms, backend)| {
                Action::MouseDrag {
                    from,
                    to,
                    button,
                    curve,
                    duration_ms,
                    backend,
                }
            }),
    )
}

#[test]
fn action_mouse_scroll_round_trips_1000_cases() -> Result<(), Box<dyn std::error::Error>> {
    run_action_round_trip(
        "mouse_scroll",
        (
            -10_000i32..=10_000,
            -10_000i32..=10_000,
            prop::option::of(point_strategy()),
            backend_strategy(),
        )
            .prop_map(|(dy, dx, at, backend)| Action::MouseScroll {
                dy,
                dx,
                at,
                backend,
            }),
    )
}

#[test]
fn action_pad_button_round_trips_1000_cases() -> Result<(), Box<dyn std::error::Error>> {
    run_action_round_trip(
        "pad_button",
        (
            0u8..=3,
            pad_button_strategy(),
            button_action_strategy(),
            0u32..=30_000,
        )
            .prop_map(|(pad, button, action, hold_ms)| Action::PadButton {
                pad,
                button,
                action,
                hold_ms,
            }),
    )
}

#[test]
fn action_pad_stick_round_trips_1000_cases() -> Result<(), Box<dyn std::error::Error>> {
    run_action_round_trip(
        "pad_stick",
        (
            0u8..=3,
            stick_strategy(),
            normalized_axis_strategy(),
            normalized_axis_strategy(),
        )
            .prop_map(|(pad, stick, x, y)| Action::PadStick { pad, stick, x, y }),
    )
}

#[test]
fn action_pad_trigger_round_trips_1000_cases() -> Result<(), Box<dyn std::error::Error>> {
    run_action_round_trip(
        "pad_trigger",
        (0u8..=3, trigger_strategy(), trigger_value_strategy()).prop_map(
            |(pad, trigger, value)| Action::PadTrigger {
                pad,
                trigger,
                value,
            },
        ),
    )
}

#[test]
fn action_pad_report_round_trips_1000_cases() -> Result<(), Box<dyn std::error::Error>> {
    run_action_round_trip(
        "pad_report",
        (0u8..=3, gamepad_report_strategy())
            .prop_map(|(pad, report)| Action::PadReport { pad, report }),
    )
}

#[test]
fn action_aim_at_round_trips_1000_cases() -> Result<(), Box<dyn std::error::Error>> {
    run_action_round_trip(
        "aim_at",
        (
            aim_target_strategy(),
            aim_style_strategy(),
            0u32..=1_000,
            backend_strategy(),
        )
            .prop_map(|(target, style, deadline_ms, backend)| Action::AimAt {
                target,
                style,
                deadline_ms,
                backend,
            }),
    )
}

#[test]
fn action_combo_round_trips_1000_cases() -> Result<(), Box<dyn std::error::Error>> {
    run_action_round_trip(
        "combo",
        (vec(combo_step_strategy(), 0..=8), backend_strategy())
            .prop_map(|(steps, backend)| Action::Combo { steps, backend }),
    )
}

#[test]
fn action_release_all_round_trips_1000_cases() -> Result<(), Box<dyn std::error::Error>> {
    run_action_round_trip("release_all", Just(Action::ReleaseAll))
}

fn run_action_round_trip<S>(
    variant: &'static str,
    strategy: S,
) -> Result<(), Box<dyn std::error::Error>>
where
    S: Strategy<Value = Action>,
{
    let config = Config {
        cases: 1_000,
        failure_persistence: None,
        ..Config::default()
    };
    let algorithm = config.rng_algorithm;
    let mut runner = TestRunner::new_with_rng(config, TestRng::deterministic_rng(algorithm));

    runner.run(&strategy, |action| {
        let json = serde_json::to_string(&action)?;
        let parsed = serde_json::from_str::<Action>(&json)?;
        prop_assert_eq!(parsed, action);
        Ok(())
    })?;

    println!("source_of_truth=action_serde edge={variant} final_value=ok cases=1000");
    Ok(())
}

fn backend_strategy() -> impl Strategy<Value = Backend> {
    prop_oneof![
        Just(Backend::Software),
        Just(Backend::Vigem),
        Just(Backend::Hardware),
        Just(Backend::Auto),
    ]
}

fn key_strategy() -> impl Strategy<Value = Key> {
    (key_code_strategy(), any::<bool>()).prop_map(|(code, use_scancode)| Key { code, use_scancode })
}

fn key_code_strategy() -> impl Strategy<Value = KeyCode> {
    prop_oneof![
        short_ascii_string_strategy().prop_map(|value| KeyCode::Named { value }),
        prop_oneof![Just('@'), Just(' '), Just('\n')].prop_map(|value| KeyCode::Symbol { value }),
        any::<u8>().prop_map(|value| KeyCode::HidCode { value }),
    ]
}

fn short_ascii_string_strategy() -> impl Strategy<Value = String> {
    vec(0u8..=25, 1..=16).prop_map(|bytes| {
        bytes
            .into_iter()
            .map(|byte| char::from(b'a' + byte))
            .collect()
    })
}

fn text_strategy() -> impl Strategy<Value = String> {
    vec(
        prop_oneof![Just('a'), Just('Z'), Just('0'), Just(' '), Just('\n')],
        0..=64,
    )
    .prop_map(|chars| chars.into_iter().collect())
}

fn dynamics_strategy() -> impl Strategy<Value = KeystrokeDynamics> {
    prop_oneof![
        Just(KeystrokeDynamics::Burst),
        (0u32..=1000).prop_map(|ms_per_char| KeystrokeDynamics::Linear { ms_per_char }),
        (0.0f32..128.0, 0.0f32..64.0, any::<bool>()).prop_map(
            |(mean_iki_ms, stddev_ms, bigram_bias)| KeystrokeDynamics::Natural {
                params: KeystrokeNaturalParams {
                    mean_iki_ms,
                    stddev_ms,
                    bigram_bias,
                },
            },
        ),
    ]
}

fn point_strategy() -> impl Strategy<Value = Point> {
    (-16_384i32..=16_384, -16_384i32..=16_384).prop_map(|(x, y)| Point { x, y })
}

fn coord_strategy() -> impl Strategy<Value = f32> {
    -4096.0f32..4096.0
}

fn normalized_axis_strategy() -> impl Strategy<Value = f32> {
    -1.0f32..1.0
}

fn trigger_value_strategy() -> impl Strategy<Value = f32> {
    0.0f32..1.0
}

fn element_id_strategy() -> impl Strategy<Value = ElementId> {
    (0u16..=u16::MAX, 1u16..=u16::MAX).prop_map(|(hwnd, runtime)| {
        ElementId::parse(&format!("0x{hwnd:x}:{runtime:x}"))
            .unwrap_or_else(|err| panic!("generated element id should parse: {err}"))
    })
}

fn mouse_target_strategy() -> impl Strategy<Value = MouseTarget> {
    prop_oneof![
        point_strategy().prop_map(|point| MouseTarget::Screen { point }),
        element_id_strategy().prop_map(|element_id| MouseTarget::Element { element_id }),
    ]
}

fn aim_target_strategy() -> impl Strategy<Value = AimTarget> {
    prop_oneof![
        point_strategy().prop_map(|point| AimTarget::Screen { point }),
        element_id_strategy().prop_map(|element_id| AimTarget::Element { element_id }),
        any::<u64>().prop_map(|track_id| AimTarget::Track { track_id }),
    ]
}

fn aim_curve_strategy() -> impl Strategy<Value = AimCurve> {
    prop_oneof![
        Just(AimCurve::Instant),
        Just(AimCurve::Linear),
        Just(AimCurve::EaseInOut),
        ((0.0f32..1.0, 0.0f32..1.0), (0.0f32..1.0, 0.0f32..1.0))
            .prop_map(|(p1, p2)| AimCurve::Bezier { p1, p2 }),
        (
            0.0f32..0.5,
            0.0f32..1.0,
            0.0f32..1.0,
            (1.0f32..1.5, 1.5f32..2.0),
            0u8..=4,
            0.0f32..10.0,
            prop::option::of(any::<u64>()),
        )
            .prop_map(
                |(
                    control_point_jitter,
                    tremor_stddev_px,
                    overshoot_prob,
                    overshoot_factor_range,
                    micro_correct_steps,
                    timing_stddev_ms,
                    seed,
                )| AimCurve::Natural {
                    params: AimNaturalParams {
                        control_point_jitter,
                        tremor_stddev_px,
                        overshoot_prob,
                        overshoot_factor_range,
                        micro_correct_steps,
                        timing_stddev_ms,
                        seed,
                    },
                },
            ),
    ]
}

fn mouse_button_strategy() -> impl Strategy<Value = MouseButton> {
    prop_oneof![
        Just(MouseButton::Left),
        Just(MouseButton::Right),
        Just(MouseButton::Middle),
        Just(MouseButton::X1),
        Just(MouseButton::X2),
    ]
}

fn button_action_strategy() -> impl Strategy<Value = ButtonAction> {
    prop_oneof![
        Just(ButtonAction::Press),
        Just(ButtonAction::Down),
        Just(ButtonAction::Up),
    ]
}

fn pad_button_strategy() -> impl Strategy<Value = PadButton> {
    prop_oneof![
        Just(PadButton::A),
        Just(PadButton::B),
        Just(PadButton::X),
        Just(PadButton::Y),
        Just(PadButton::Lb),
        Just(PadButton::Rb),
        Just(PadButton::Ls),
        Just(PadButton::Rs),
        Just(PadButton::Back),
        Just(PadButton::Start),
        Just(PadButton::Up),
        Just(PadButton::Down),
        Just(PadButton::Left),
        Just(PadButton::Right),
        Just(PadButton::Guide),
    ]
}

fn stick_strategy() -> impl Strategy<Value = Stick> {
    prop_oneof![Just(Stick::Left), Just(Stick::Right)]
}

fn trigger_strategy() -> impl Strategy<Value = Trigger> {
    prop_oneof![Just(Trigger::Left), Just(Trigger::Right)]
}

fn aim_style_strategy() -> impl Strategy<Value = AimStyle> {
    prop_oneof![
        Just(AimStyle::Snap),
        Just(AimStyle::Flick),
        Just(AimStyle::Natural),
        Just(AimStyle::Track),
    ]
}

fn gamepad_report_strategy() -> impl Strategy<Value = GamepadReport> {
    (
        vec(pad_button_strategy(), 0..=8),
        normalized_axis_strategy(),
        normalized_axis_strategy(),
        normalized_axis_strategy(),
        normalized_axis_strategy(),
        trigger_value_strategy(),
        trigger_value_strategy(),
    )
        .prop_map(
            |(buttons, thumb_l_x, thumb_l_y, thumb_r_x, thumb_r_y, lt, rt)| GamepadReport {
                buttons,
                thumb_l: (thumb_l_x, thumb_l_y),
                thumb_r: (thumb_r_x, thumb_r_y),
                lt,
                rt,
            },
        )
}

fn combo_step_strategy() -> impl Strategy<Value = ComboStep> {
    (0u32..=1_000, combo_input_strategy()).prop_map(|(at_ms, input)| ComboStep { at_ms, input })
}

fn combo_input_strategy() -> impl Strategy<Value = ComboInput> {
    prop_oneof![
        key_strategy().prop_map(|key| ComboInput::KeyDown { key }),
        key_strategy().prop_map(|key| ComboInput::KeyUp { key }),
        (key_strategy(), 0u16..=30_000)
            .prop_map(|(key, hold_ms)| ComboInput::KeyPress { key, hold_ms }),
        (mouse_button_strategy(), button_action_strategy())
            .prop_map(|(button, action)| { ComboInput::MouseButton { button, action } }),
        (coord_strategy(), coord_strategy())
            .prop_map(|(dx, dy)| ComboInput::MouseMoveRel { dx, dy }),
        (0u8..=3, pad_button_strategy(), button_action_strategy()).prop_map(
            |(pad, button, action)| ComboInput::PadButton {
                pad,
                button,
                action,
            },
        ),
        (
            0u8..=3,
            stick_strategy(),
            normalized_axis_strategy(),
            normalized_axis_strategy(),
        )
            .prop_map(|(pad, stick, x, y)| ComboInput::PadStick { pad, stick, x, y }),
    ]
}
