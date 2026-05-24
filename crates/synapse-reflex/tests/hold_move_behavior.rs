use std::time::Duration;

use chrono::Utc;
use synapse_action::ActionHandle;
use synapse_core::{
    Action, Backend, ButtonAction, Event, EventFilter, EventSource, Key, KeyCode, MouseButton,
    ReflexButtonTarget, ReflexLifetime, error_codes,
};
use synapse_reflex::{
    EventBus, HoldButtonController, HoldButtonOutput, HoldButtonParams, HoldLifetimeContext,
    HoldMoveController, HoldMoveOutput, HoldMoveParams, HoldMovePhase,
    REFLEX_LIFETIME_EXPIRED_KIND, ReflexError,
};
use tokio::sync::mpsc;

#[test]
fn hold_move_until_event_releases_key_once() -> Result<(), Box<dyn std::error::Error>> {
    let key = named_key("w");
    let lifetime = ReflexLifetime::UntilEvent {
        filter: EventFilter::Kind {
            kind: "stop".to_owned(),
        },
    };
    let mut controller =
        HoldMoveController::new("hold-event", HoldMoveParams::new(key.clone()), lifetime)?;
    let bus = EventBus::default();
    let (handle, mut rx) = ActionHandle::channel();

    let before = drain(&mut rx);
    let registered = controller.register_dispatch(&handle)?;
    let after_register = drain(&mut rx);
    let error =
        controller.step_dispatch(&context(2_000, &[event("stop", 1)], false), &handle, &bus);
    let after_event = drain(&mut rx);
    let duplicate = controller.step_dispatch(&context(1, &[], false), &handle, &bus)?;
    let after_duplicate = drain(&mut rx);

    assert!(before.is_empty());
    assert_eq!(registered, HoldMoveOutput::Registered { actions: 1 });
    assert_eq!(
        after_register,
        vec![Action::KeyDown {
            key: key.clone(),
            backend: Backend::Software,
        }]
    );
    assert!(matches!(
        error,
        Err(ReflexError::LifetimeExpired { ref reflex_id }) if reflex_id == "hold-event"
    ));
    assert_eq!(
        after_event,
        vec![Action::KeyUp {
            key,
            backend: Backend::Software,
        }]
    );
    assert_eq!(
        duplicate,
        HoldMoveOutput::Idle {
            reason: "not_holding"
        }
    );
    assert!(after_duplicate.is_empty());
    Ok(())
}

#[test]
fn hold_move_zero_duration_releases_immediately() -> Result<(), Box<dyn std::error::Error>> {
    let key = named_key("w");
    let mut controller = HoldMoveController::new(
        "hold-zero",
        HoldMoveParams::new(key.clone()),
        ReflexLifetime::Duration { ms: 0 },
    )?;
    let bus = EventBus::default();
    let (handle, mut rx) = ActionHandle::channel();

    controller.register_dispatch(&handle)?;
    let down = drain(&mut rx);
    let result = controller.step_dispatch(&context(0, &[], false), &handle, &bus);
    let up = drain(&mut rx);

    assert!(matches!(result, Err(ReflexError::LifetimeExpired { .. })));
    assert_eq!(
        down,
        vec![Action::KeyDown {
            key: key.clone(),
            backend: Backend::Software,
        }]
    );
    assert_eq!(
        up,
        vec![Action::KeyUp {
            key,
            backend: Backend::Software,
        }]
    );
    assert_eq!(controller.phase(), HoldMovePhase::Released);
    Ok(())
}

#[test]
fn hold_move_external_cancel_releases_until_cancelled() -> Result<(), Box<dyn std::error::Error>> {
    let key = named_key("w");
    let mut controller = HoldMoveController::new(
        "hold-cancel",
        HoldMoveParams::new(key.clone()),
        ReflexLifetime::UntilCancelled,
    )?;
    let bus = EventBus::default();
    let (handle, mut rx) = ActionHandle::channel();

    controller.register_dispatch(&handle)?;
    let after_register = drain(&mut rx);
    let result = controller.step_dispatch(&context(100, &[], true), &handle, &bus);
    let after_cancel = drain(&mut rx);

    assert!(matches!(result, Err(ReflexError::LifetimeExpired { .. })));
    assert_eq!(
        after_register,
        vec![Action::KeyDown {
            key: key.clone(),
            backend: Backend::Software,
        }]
    );
    assert_eq!(
        after_cancel,
        vec![Action::KeyUp {
            key,
            backend: Backend::Software,
        }]
    );
    Ok(())
}

#[test]
fn hold_move_safety_cap_expires_after_held_key_limit() -> Result<(), Box<dyn std::error::Error>> {
    let key = named_key("w");
    let mut controller = HoldMoveController::new(
        "hold-cap",
        HoldMoveParams::new(key.clone()),
        ReflexLifetime::UntilCancelled,
    )?;
    let bus = EventBus::default();
    let subscriber = bus.subscribe(
        EventFilter::Kind {
            kind: REFLEX_LIFETIME_EXPIRED_KIND.to_owned(),
        },
        Vec::new(),
        false,
    )?;
    let (handle, mut rx) = ActionHandle::channel();

    controller.register_dispatch(&handle)?;
    let down = drain(&mut rx);
    let result = controller.step_dispatch(&context(30_001, &[], false), &handle, &bus);
    let up = drain(&mut rx);
    let events = subscriber.drain();

    assert!(matches!(result, Err(ReflexError::LifetimeExpired { .. })));
    assert_eq!(
        down,
        vec![Action::KeyDown {
            key: key.clone(),
            backend: Backend::Software,
        }]
    );
    assert_eq!(
        up,
        vec![Action::KeyUp {
            key,
            backend: Backend::Software,
        }]
    );
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data["code"], error_codes::REFLEX_LIFETIME_EXPIRED);
    assert_eq!(events[0].data["reason"], "safety_cap");
    Ok(())
}

#[test]
fn hold_button_mouse_uses_down_up_actions() -> Result<(), Box<dyn std::error::Error>> {
    let mut controller = HoldButtonController::new(
        "button-duration",
        HoldButtonParams::new(ReflexButtonTarget::Mouse {
            button: MouseButton::Left,
        }),
        ReflexLifetime::Duration { ms: 10 },
    )?;
    let bus = EventBus::default();
    let (handle, mut rx) = ActionHandle::channel();

    let registered = controller.register_dispatch(&handle)?;
    let after_register = drain(&mut rx);
    let result = controller.step_dispatch(&context(10, &[], false), &handle, &bus);
    let after_duration = drain(&mut rx);

    assert_eq!(registered, HoldButtonOutput::Registered);
    assert!(matches!(result, Err(ReflexError::LifetimeExpired { .. })));
    assert_eq!(
        after_register,
        vec![Action::MouseButton {
            button: MouseButton::Left,
            action: ButtonAction::Down,
            hold_ms: 0,
            backend: Backend::Software,
        }]
    );
    assert_eq!(
        after_duration,
        vec![Action::MouseButton {
            button: MouseButton::Left,
            action: ButtonAction::Up,
            hold_ms: 0,
            backend: Backend::Software,
        }]
    );
    Ok(())
}

const fn context(elapsed_ms: u64, events: &[Event], cancelled: bool) -> HoldLifetimeContext<'_> {
    HoldLifetimeContext {
        tick_elapsed: Duration::from_millis(elapsed_ms),
        events,
        cancelled,
    }
}

fn drain(rx: &mut mpsc::Receiver<synapse_action::ActionMessage>) -> Vec<Action> {
    let mut actions = Vec::new();
    while let Ok((action, _ack)) = rx.try_recv() {
        actions.push(action);
    }
    actions
}

fn event(kind: &str, seq: u64) -> Event {
    Event {
        seq,
        at: Utc::now(),
        source: EventSource::Reflex,
        kind: kind.to_owned(),
        data: serde_json::json!({}),
        correlations: Vec::new(),
    }
}

fn named_key(value: &str) -> Key {
    Key {
        code: KeyCode::Named {
            value: value.to_owned(),
        },
        use_scancode: false,
    }
}
