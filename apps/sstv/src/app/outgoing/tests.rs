use std::time::Duration;

use rstest::rstest;

use super::*;
use crate::test_util::refused_port;

fn picture(phase: TxPhase) -> Outgoing {
    let mut outgoing = Outgoing::new(1.0);
    outgoing.begin_for_test(Kind::Picture, phase);
    outgoing
}

fn advance(outgoing: &mut Outgoing, rig: &RigSnapshot) -> Option<Ending> {
    outgoing.running.as_mut()?.advance(rig)
}

fn rig_in(state: RigState) -> RigSnapshot {
    RigSnapshot {
        state,
        ..RigSnapshot::default()
    }
}

#[rstest]
#[case::nothing_was_keyed(false, RigState::Receiving, true)]
#[case::still_switching_over(true, RigState::Receiving, false)]
#[case::keyed_and_settled(true, RigState::Transmitting, true)]
fn audio_waits_until_the_rig_says_it_is_transmitting(
    #[case] keyed: bool,
    #[case] state: RigState,
    #[case] expected: bool,
) {
    let mut outgoing = picture(TxPhase::Producing);
    if keyed {
        outgoing.key_for_test();
    }

    assert_eq!(outgoing.running.as_ref().unwrap().ready_to_send(state), expected);
}

#[test]
fn playback_is_only_started_once_the_keyed_rig_is_transmitting() {
    let mut outgoing = picture(TxPhase::Producing);
    outgoing.key_for_test();

    assert_eq!(advance(&mut outgoing, &rig_in(RigState::Receiving)), None);
    assert!(!outgoing.running.as_ref().unwrap().started);

    let ending = advance(&mut outgoing, &rig_in(RigState::Transmitting));

    assert_eq!(ending, Some(Ending::Failed(AppError::PlaybackClosed.to_string())));
}

#[test]
fn playback_waits_for_the_worker_to_prime_the_queue() {
    let mut outgoing = picture(TxPhase::Priming);

    assert_eq!(advance(&mut outgoing, &rig_in(RigState::Receiving)), None);
    assert!(!outgoing.running.as_ref().unwrap().started);
}

#[test]
fn a_keyed_rig_that_reports_an_error_fails_the_transmission() {
    let mut outgoing = picture(TxPhase::Producing);
    outgoing.key_for_test();
    let rig = RigSnapshot {
        error: Some(refused_port()),
        ..rig_in(RigState::Transmitting)
    };

    let ending = advance(&mut outgoing, &rig);

    assert!(
        matches!(&ending, Some(Ending::Failed(error)) if error.contains("connection refused")),
        "{ending:?}"
    );
}

#[test]
fn a_rig_error_is_nothing_to_a_transmission_that_did_not_key_it() {
    let mut outgoing = picture(TxPhase::Priming);
    let rig = RigSnapshot {
        error: Some(refused_port()),
        ..rig_in(RigState::Failed)
    };

    assert_eq!(advance(&mut outgoing, &rig), None);
}

#[rstest]
#[case::failed(TxPhase::Failed, Ending::Failed(AppError::WorkerUnavailable("transmit").to_string()))]
#[case::cancelled(TxPhase::Cancelled, Ending::Cancelled)]
fn what_the_worker_reports_ends_the_transmission(#[case] phase: TxPhase, #[case] expected: Ending) {
    let mut outgoing = picture(phase);

    assert_eq!(advance(&mut outgoing, &rig_in(RigState::Disconnected)), Some(expected));
}

#[test]
fn the_worker_s_own_error_is_the_one_reported() {
    let mut outgoing = picture(TxPhase::Failed);
    outgoing.running.as_mut().unwrap().snapshot.error = Some(AppError::PlaybackUnderrun);

    assert_eq!(
        advance(&mut outgoing, &rig_in(RigState::Disconnected)),
        Some(Ending::Failed(AppError::PlaybackUnderrun.to_string()))
    );
}

#[test]
fn a_tune_tone_runs_until_its_deadline() {
    let now = Instant::now();
    let mut outgoing = Outgoing::new(1.0);
    outgoing.begin_for_test(
        Kind::Tune {
            until: now + Duration::from_secs(1),
        },
        TxPhase::Producing,
    );

    assert_eq!(outgoing.poll(&RigSnapshot::default(), now), None);
    assert_eq!(
        outgoing.poll(&RigSnapshot::default(), now + Duration::from_secs(1)),
        Some(Ending::Complete)
    );
}

#[rstest]
#[case::complete(Ending::Complete)]
#[case::cancelled(Ending::Cancelled)]
#[case::failed(Ending::Failed("broken".to_owned()))]
fn every_ending_gives_the_keying_back(#[case] ending: Ending) {
    let mut outgoing = picture(TxPhase::Producing);
    outgoing.key_for_test();

    outgoing.end(&ending, None);

    assert!(!outgoing.is_active());
    assert!(!outgoing.is_keyed());
    assert_eq!(outgoing.is_complete(), ending == Ending::Complete);
}

#[test]
fn releasing_the_rig_keeps_the_audio_going() {
    let mut outgoing = picture(TxPhase::Producing);
    outgoing.key_for_test();

    outgoing.release_rig(None);

    assert!(outgoing.is_active());
    assert!(!outgoing.is_keyed());
    assert!(outgoing.running.as_ref().unwrap().ready_to_send(RigState::Receiving));
}

#[test]
fn a_new_transmission_forgets_how_the_last_one_ended() {
    let mut outgoing = picture(TxPhase::Producing);
    outgoing.end(&Ending::Complete, None);
    assert_eq!(outgoing.progress(), TxProgress::Complete);

    outgoing.begin_for_test(Kind::Picture, TxPhase::Priming);

    assert!(!outgoing.is_complete());
    assert_eq!(outgoing.progress(), TxProgress::Leader);
}

#[rstest]
#[case::picture(Kind::Picture, false, true)]
#[case::tone(Kind::Tune { until: Instant::now() }, true, false)]
fn the_kind_says_what_is_being_sent(#[case] kind: Kind, #[case] tuning: bool, #[case] picture: bool) {
    let mut outgoing = Outgoing::new(1.0);
    outgoing.begin_for_test(kind, TxPhase::Producing);

    assert_eq!(outgoing.is_tuning(), tuning);
    assert_eq!(outgoing.is_sending_picture(), picture);
}
