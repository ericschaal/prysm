use super::*;
use Availability::{Active, Blocked, Down, Ready, Waiting};

#[test]
fn starting_requires_two_ready_adapters_and_an_idle_owner() {
    for capture in [Down, Ready, Active, Waiting, Blocked] {
        for output in [Down, Ready, Active, Waiting, Blocked] {
            let mut session = SessionLifecycle::default();
            let ready = capture == Ready && output == Ready;
            assert_eq!(session.start(capture, output), ready);
            if ready {
                assert!(!session.start(Ready, Ready));
                session.capture_opened();
                assert!(!session.start(Ready, Ready));
                session.output_opened();
                assert!(!session.start(Ready, Ready));
            }
        }
    }
}

#[test]
fn only_an_unsent_idle_action_can_change_and_only_join_allows_reopening() {
    for already_sent in [false, true] {
        let mut session = SessionLifecycle::default();
        assert!(session.start(Ready, Ready));
        assert!(session.stop(OutputRelease::Off, false));
        if already_sent {
            assert_eq!(session.take_idle(), Some(OutputRelease::Off));
        }
        assert!(!session.stop(OutputRelease::Preserve, true));
        assert_eq!(
            session.take_idle(),
            (!already_sent).then_some(OutputRelease::Preserve)
        );
        assert_eq!(session.take_idle(), None);
        assert!(!session.start(Ready, Ready));
        assert!(session.released());
        assert!(session.start(Ready, Ready));
    }
}

#[test]
fn unconfirmed_release_remains_exclusive() {
    let mut session = SessionLifecycle::default();
    assert!(session.start(Ready, Ready));
    session.stop(OutputRelease::Off, true);
    session.fault(FaultPhase::SourceUnreleased);
    assert!(!session.released());
    assert!(!session.output_opened());
    assert!(!session.capture_opened());
    session.start_failed();
    assert!(!session.start(Ready, Ready));
    assert_eq!(session.take_idle(), None);
    assert_eq!(
        session.phase(),
        SessionPhase::Faulted(FaultPhase::SourceUnreleased)
    );
}

#[test]
fn late_open_notifications_cannot_revive_a_stopping_session() {
    let mut session = SessionLifecycle::default();
    assert!(session.start(Ready, Ready));
    assert!(!session.output_opened());
    assert!(session.stop(OutputRelease::Off, false));
    assert!(!session.capture_opened());
    assert!(!session.output_opened());
    session.start_failed();
    assert_eq!(session.take_idle(), Some(OutputRelease::Off));
    assert!(session.stopping());
    assert!(!session.released());
    assert!(session.start(Ready, Ready));
    // A duplicate release from the previous stop cannot cancel this open.
    assert!(!session.released());
    assert!(session.capture_opened());
}
