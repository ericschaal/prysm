use super::*;
use prysm_ports::OutputObservation;

fn fact(power: Power, display: OutputDisplay) -> OutputObservationState {
    OutputObservationState::Observed(OutputObservation { power, display })
}

#[test]
fn boot_reconciles_all_power_display_and_memory_combinations() {
    use OutputDisplay::{Idle, Live, Override};
    for saved in [None, Some(Mode::Effects), Some(Mode::ScreenSync)] {
        for power in [Power::Off, Power::On] {
            for display in [Idle, Live, Override] {
                let mut intent = LightingIntent::new(saved, true);
                intent.observe_startup(fact(power, display));
                let expected_memory = if display == Override {
                    Mode::Effects
                } else {
                    saved.unwrap_or(Mode::Effects)
                };
                let expected = match (power, display) {
                    (Power::Off, _) => Selection::Off,
                    (Power::On, Live) => Selection::External,
                    (Power::On, _) => Selection::Mode(expected_memory),
                };
                assert_eq!(intent.physical_power(), Some(power));
                assert_eq!(intent.remembered(), expected_memory);
                assert_eq!(intent.selection(), expected);
                assert_eq!(
                    intent.next_action(Availability::Ready),
                    (expected == Selection::Mode(Mode::ScreenSync))
                        .then_some(IntentAction::StartSync)
                );
                assert_eq!(
                    intent.sync_takeover(),
                    (expected == Selection::Mode(Mode::ScreenSync)).then_some(false)
                );
            }
        }
    }
}

#[test]
fn power_or_cancellation_outranks_a_late_startup_reply() {
    for first in [
        OutputObservationState::Pending,
        OutputObservationState::Unsupported,
    ] {
        for cancel in [true, false] {
            let mut intent = LightingIntent::new(Some(Mode::ScreenSync), true);
            intent.observe_startup(first);
            assert_eq!(intent.selection(), Selection::Off);
            intent.press_power(SessionPhase::Idle);
            if cancel {
                intent.pause();
            }
            intent.observe_startup(fact(Power::Off, OutputDisplay::Override));
            assert_eq!(intent.remembered(), Mode::ScreenSync);
            assert_eq!(intent.physical_power(), Some(Power::Off));
            assert_eq!(
                intent.next_action(Availability::Ready),
                (!cancel).then_some(IntentAction::StartSync)
            );
        }
    }
}

#[test]
fn effects_waits_for_output_only_and_does_not_enforce_a_fulfilled_request() {
    let mut intent = LightingIntent::new(None, true);
    intent.press_power(SessionPhase::Idle);
    assert_eq!(intent.next_action(Availability::Waiting), None);
    assert_eq!(
        intent.next_action(Availability::Ready),
        Some(IntentAction::ApplyEffects)
    );
    intent.press_power(SessionPhase::Idle);
    assert_eq!(intent.selection(), Selection::Mode(Mode::Effects));
    intent.command_result(Err(Error::other("unanswered")), Availability::Waiting);
    assert_eq!(intent.next_action(Availability::Waiting), None);
    assert_eq!(
        intent.next_action(Availability::Ready),
        Some(IntentAction::ApplyEffects)
    );
    intent.command_result(Ok(()), Availability::Ready);
    assert!(intent.error().is_none());
    intent.observe_startup(fact(Power::Off, OutputDisplay::Idle));
    assert_eq!(intent.next_action(Availability::Ready), None);
}

#[test]
fn off_timeout_does_not_queue_another_off_or_restore_sync() {
    let mut intent = LightingIntent::new(Some(Mode::ScreenSync), true);
    intent.observe_startup(fact(Power::On, OutputDisplay::Live));
    intent.press_power(SessionPhase::Idle);
    assert_eq!(
        intent.next_action(Availability::Waiting),
        Some(IntentAction::SendOff)
    );
    assert!(intent.control(SessionPhase::Idle).busy);
    intent.press_power(SessionPhase::Idle);
    intent.command_result(
        Err(Error::other("no acknowledgment")),
        Availability::Waiting,
    );
    assert!(!intent.control(SessionPhase::Idle).busy);
    assert!(intent.error().is_some());
    assert_eq!(intent.physical_power(), None);
    assert_eq!(intent.selection(), Selection::Off);
    assert_eq!(intent.next_action(Availability::Ready), None);
    intent.press_power(SessionPhase::Idle);
    assert_eq!(
        intent.next_action(Availability::Ready),
        Some(IntentAction::StartSync)
    );
}

#[test]
fn only_latest_selection_enters_after_exit_and_fault_prevents_entry() {
    for released in [false, true] {
        let mut intent = LightingIntent::new(None, true);
        intent.select(Mode::Effects, SessionPhase::Idle);
        assert_eq!(intent.next_action(Availability::Waiting), None);
        intent.press_power(SessionPhase::Idle);
        assert_eq!(
            intent.next_action(Availability::Ready),
            Some(IntentAction::ExitMode)
        );
        intent.select(Mode::ScreenSync, SessionPhase::Idle);
        assert_eq!(intent.next_action(Availability::Ready), None);
        if released {
            intent.exited();
        } else {
            intent.fault(Error::other("unreleased"));
        }
        assert_eq!(
            intent.next_action(Availability::Ready),
            released.then_some(IntentAction::StartSync)
        );
    }
}

#[test]
fn blocked_outranks_waiting_and_clearing_the_blocker_keeps_power_off() {
    let mut intent = LightingIntent::new(None, true);
    intent.select(Mode::Effects, SessionPhase::Idle);
    assert_eq!(intent.next_action(Availability::Waiting), None);
    assert_eq!(
        intent.next_action(Availability::Blocked),
        Some(IntentAction::ExitMode)
    );
    intent.exited();
    intent.configuration_changed();
    assert_eq!(intent.selection(), Selection::Off);
    assert_eq!(intent.next_action(Availability::Ready), None);
}

#[test]
fn explicit_and_observed_effects_handoff_preserve_the_same_display() {
    for observed in [false, true] {
        let mut intent = LightingIntent::new(Some(Mode::ScreenSync), true);
        intent.select(Mode::ScreenSync, SessionPhase::Idle);
        assert_eq!(
            intent.next_action(Availability::Ready),
            Some(IntentAction::StartSync)
        );
        intent.sync_started();
        if observed {
            intent.handed_off();
        } else {
            intent.select(Mode::Effects, SessionPhase::Running);
        }
        assert_eq!(
            intent.next_action(Availability::Ready),
            Some(IntentAction::ExitMode)
        );
        intent.exited();
        assert_eq!(intent.next_action(Availability::Ready), None);
        assert_eq!(intent.remembered(), Mode::Effects);
        assert_eq!(intent.selection(), Selection::Mode(Mode::Effects));
    }
}

#[test]
fn screen_sync_only_output_ignores_effects_and_defaults_to_screen_sync() {
    let mut intent = LightingIntent::new(Some(Mode::Effects), false);
    intent.select(Mode::Effects, SessionPhase::Idle);
    assert_eq!(intent.selection(), Selection::Off);
    intent.press_power(SessionPhase::Idle);
    assert_eq!(intent.remembered(), Mode::ScreenSync);
    assert_eq!(intent.selection(), Selection::Mode(Mode::ScreenSync));
}
