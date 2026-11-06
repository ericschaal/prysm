//! One retained blocking worker serializes screen writes and restores the terminal.
use crate::{
    CLOSED, Command, Reply, TerminalOwner,
    mailbox::Mailbox,
    screen::{Display, Mode, View, restore_result},
};
use futures::StreamExt;
use prysm_contract::{
    ControlError, OwnerStopped,
    output::{IdleOutputMode, OutputApplied},
};
use std::{
    future::Future,
    io,
    sync::Arc,
    thread::JoinHandle,
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

const REDRAW: Duration = Duration::from_millis(100);

pub(super) async fn run<D: Display>(
    mut owner: TerminalOwner,
    shutdown: impl Future<Output = ()> + Send,
    open: impl FnOnce() -> Result<D, ControlError> + Send + 'static,
) -> Result<OwnerStopped, ControlError> {
    let mailbox = owner.mailbox.clone();
    let preview = owner.preview.take();
    let (done, mut finished) = oneshot::channel();
    let join = std::thread::Builder::new()
        .name("prysm-terminal".into())
        .spawn(move || {
            let result = drive(owner, open);
            let _ = done.send(());
            result
        })
        .map_err(|error| ControlError::Adapter(Box::new(error)))?;
    let mut worker = Worker {
        join: Some(join),
        mailbox,
        preview,
    };
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => {
                worker.preview = None;
                worker.mailbox.request_shutdown();
                let _ = finished.await;
                break;
            }
            _ = &mut finished => break,
            _ = worker.mailbox.preview_stopped(), if worker.preview.is_some() => worker.preview = None,
            item = async { worker.preview.as_mut().expect("preview branch enabled").next().await }, if worker.preview.is_some() => {
                match item {
                    Some(Ok(frame)) => {
                        if !worker.mailbox.offer_preview(frame) { worker.preview = None; }
                        tokio::task::consume_budget().await;
                    }
                    failed => {
                        worker.preview = None;
                        let reason = failed.and_then(Result::err).map_or_else(|| "preview source closed".into(), |error| error.to_string());
                        worker.mailbox.disable_preview(reason);
                    }
                }
            }
        }
    }
    worker.preview = None;
    worker.join()
}

struct Worker {
    join: Option<JoinHandle<Result<OwnerStopped, ControlError>>>,
    mailbox: Arc<Mailbox>,
    preview: Option<prysm_contract::preview::PreviewLease>,
}
impl Worker {
    fn join(&mut self) -> Result<OwnerStopped, ControlError> {
        self.join
            .take()
            .expect("retained terminal worker")
            .join()
            .map_err(|_| {
                ControlError::RecoveryRequired(Box::new(io::Error::other(
                    "terminal worker panicked",
                )))
            })?
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.preview = None;
        if self.join.is_some() {
            self.mailbox.request_shutdown();
            // Cancellation fallback only; normal shutdown waits asynchronously first.
            let _ = self.join();
        }
    }
}

fn drive<D: Display>(
    mut owner: TerminalOwner,
    open: impl FnOnce() -> Result<D, ControlError>,
) -> Result<OwnerStopped, ControlError> {
    owner.mailbox.register_worker();
    if owner.mailbox.stopping() {
        return Ok(OwnerStopped);
    }
    let mut display = open()?;
    let mut view = View::default();
    let result = render_loop(&mut owner, &mut display, &mut view);
    owner.mailbox.request_shutdown();
    owner.commands.close();
    while let Ok(command) = owner.commands.try_recv() {
        let _ = reply(command).send(Err(CLOSED));
    }
    let restored = display.restore();
    restore_result(result.map(|()| OwnerStopped), restored)
}

fn render_loop(
    owner: &mut TerminalOwner,
    display: &mut impl Display,
    view: &mut View,
) -> Result<(), ControlError> {
    let mut redraw = Instant::now();
    while !owner.mailbox.stopping() {
        if let Ok(command) = owner.commands.try_recv() {
            let result = apply(&command, &owner.mailbox, view)
                .and_then(|()| draw(&owner.mailbox, display, view));
            let result = result.map(|()| {
                if let Command::Finish(session, _, _) = &command {
                    owner.mailbox.finished(*session);
                }
                publish(&owner.mailbox, view);
                OutputApplied::Sent
            });
            if let Err(ControlError::Adapter(_)) = &result {
                let failure = prysm_contract::Failure::from(result.unwrap_err());
                let _ = reply(command).send(Err(failure.clone().into()));
                return Err(failure.into());
            }
            let _ = reply(command).send(result);
            redraw = Instant::now() + REDRAW;
            continue;
        }
        if Instant::now() >= redraw {
            if let Some(frame) = owner.mailbox.take() {
                view.colors = frame.colors.clone();
                view.frame = Some(frame);
            }
            if let Err(error) = draw(&owner.mailbox, display, view)
                && (!matches!(error, ControlError::Unavailable(_)) || !owner.mailbox.stopping())
            {
                return Err(error);
            }
            redraw = Instant::now() + REDRAW;
        }
        std::thread::park_timeout(redraw.saturating_duration_since(Instant::now()));
    }
    Ok(())
}

fn draw(
    mailbox: &Mailbox,
    display: &mut impl Display,
    view: &mut View,
) -> Result<(), ControlError> {
    view.info = mailbox.info();
    view.preview = mailbox.preview();
    mailbox.authorize()?;
    display
        .draw(view)
        .map_err(|e| ControlError::Adapter(Box::new(e)))?;
    if let Some(reason) = display.preview_failure() {
        mailbox.disable_preview(reason);
    }
    Ok(())
}

fn publish(mailbox: &Mailbox, view: &View) {
    let mut state = Mailbox::unknown();
    state.held = view.mode == Mode::Held;
    state.admitted = view.admitted;
    if view.mode == Mode::Off {
        state.idle_mode = Some(IdleOutputMode::Off);
    }
    state.confirmation = view.settings.as_ref().map(|_| OutputApplied::Sent);
    mailbox.publish(state);
}

fn apply(command: &Command, mailbox: &Mailbox, view: &mut View) -> Result<(), ControlError> {
    match command {
        Command::Configure(settings, _) => {
            settings.validate()?;
            mailbox.can_configure()?;
            view.settings = Some(settings.clone());
            view.mode = Mode::Configured;
        }
        _ if view.settings.is_none() => {
            return Err(ControlError::Unavailable("configure terminal output first"));
        }
        Command::Hold(epoch, colors, _) => {
            mailbox.hold(*epoch)?;
            view.colors = colors.clone();
            view.frame = None;
            view.admitted = None;
            view.mode = Mode::Held;
        }
        Command::Admit(epoch, _) => {
            mailbox.admit(*epoch)?;
            if view.admitted != Some(*epoch) {
                view.frame = None;
                if view.mode != Mode::Held {
                    view.colors = Default::default();
                }
            }
            view.admitted = Some(*epoch);
            if let Some(frame) = mailbox.take() {
                view.colors = frame.colors.clone();
                view.frame = Some(frame);
            }
            view.mode = Mode::Streaming;
        }
        Command::Finish(session, mode, _) => {
            if *mode != IdleOutputMode::Off {
                return Err(ControlError::Unavailable(
                    "terminal output has no native mode",
                ));
            }
            mailbox.finish(*session)?;
            view.colors = Default::default();
            view.frame = None;
            view.admitted = None;
            view.mode = Mode::Off;
        }
    }
    Ok(())
}

fn reply(command: Command) -> Reply {
    match command {
        Command::Configure(_, r)
        | Command::Hold(_, _, r)
        | Command::Admit(_, r)
        | Command::Finish(_, _, r) => r,
    }
}
