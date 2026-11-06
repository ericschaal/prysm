//! Command dispatch and retained session lifetime. Native details stay in backend.rs.
use crate::{Command, DeviceCaptureSettings, DeviceOwner, backend::Run, session::Session};
use prysm_contract::{
    ControlError, Failure, OwnerStopped,
    capture::{CaptureState, CaptureStopped},
};
use std::future::Future;

enum State {
    Idle,
    Live(Box<Session>),
    Blocked(Failure),
}

pub(super) async fn run(
    mut owner: DeviceOwner,
    shutdown: impl Future<Output = ()> + Send,
    mut open: impl FnMut(&DeviceCaptureSettings) -> Result<Run, ControlError> + Send,
) -> Result<OwnerStopped, ControlError> {
    let mut state = State::Idle;
    let mut highest = 0;
    let mut shutting_down = false;
    let mut commands_live = true;
    tokio::pin!(shutdown);
    loop {
        if shutting_down {
            match &state {
                State::Idle => return Ok(OwnerStopped),
                State::Blocked(failure) => return Err(failure.clone().into()),
                State::Live(_) => {}
            }
        }
        tokio::select! {
            biased;
            _ = &mut shutdown, if !shutting_down => {
                shutting_down = true;
                owner.commands.close();
                while let Ok(command) = owner.commands.try_recv() { reject(command, crate::CLOSED); }
                if let State::Live(session) = &mut state { session.stop(None); }
            }
            command = owner.commands.recv(), if commands_live && !shutting_down => {
                let Some(command) = command else { commands_live = false; continue; };
                match command {
                    Command::Start(request, reply) => {
                        let error = if request.session.instance != owner.instance {
                            Some(ControlError::Conflict("capture session belongs to another instance"))
                        } else { match &state {
                            State::Blocked(failure) => Some(failure.clone().into()),
                            State::Live(session) => Some(ControlError::CleanupPending(session.id())),
                            State::Idle if request.session.sequence.get() <= highest => Some(ControlError::Conflict("capture session is sealed")),
                            State::Idle => request.settings.validate().err(),
                        }};
                        if let Some(error) = error { let _ = reply.send(Err(error)); continue; }
                        highest = request.session.sequence.get();
                        match open(&request.settings) {
                            Ok(run) => {
                                let DeviceCaptureSettings::Device { rate, .. } = request.settings else { unreachable!("validated device") };
                                state = State::Live(Box::new(Session::new(request.session, rate, run, owner.mailbox.clone(), reply)));
                            }
                            Err(error) => {
                                let failure = Failure::from(error);
                                owner.mailbox.publish(CaptureState::Failed { session: request.session, failure: failure.clone(), cleanup_complete: true });
                                let _ = reply.send(Err(failure.into()));
                            }
                        }
                    }
                    Command::Stop(session, reply) => {
                        if session.instance != owner.instance {
                            let _ = reply.send(Err(ControlError::Conflict("capture session belongs to another instance")));
                            continue;
                        }
                        match &mut state {
                            State::Live(active) if active.id() == session => active.stop(Some(reply)),
                            State::Live(_) => { let _ = reply.send(Err(ControlError::Conflict("another capture session is active"))); }
                            State::Blocked(failure) => { let _ = reply.send(Err(failure.clone().into())); }
                            State::Idle => {
                                highest = highest.max(session.sequence.get());
                                let _ = reply.send(Ok(CaptureStopped { session }));
                            }
                        }
                    }
                }
            }
            completion = async {
                match &mut state {
                    State::Live(session) => session.progress().await,
                    _ => std::future::pending().await,
                }
            } => {
                if let Some(completion) = completion {
                    let State::Live(session) = std::mem::replace(&mut state, State::Idle) else { unreachable!("owned completion") };
                    if let Some(failure) = session.completed(completion) { state = State::Blocked(failure); }
                }
            }
        }
    }
}

fn reject(command: Command, error: ControlError) {
    match command {
        Command::Start(_, reply) => {
            let _ = reply.send(Err(error));
        }
        Command::Stop(_, reply) => {
            let _ = reply.send(Err(error));
        }
    }
}
