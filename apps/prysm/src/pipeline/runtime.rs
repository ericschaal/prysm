//! One lifecycle owner; resources remain attached until completion and join are observed.
use super::worker::{self, Configure, PipelineExit};
use crate::{config::PipelineConfig, input::InputSource};
use anyhow::{Context, Result, anyhow, bail};
use prysm_contract::{PipelinePhase, PipelineStatus};
use std::sync::{Arc, Mutex};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

type Reply = oneshot::Sender<Result<()>>;
enum Command {
    Start(Reply),
    Stop(Reply),
    Configure(Box<PipelineConfig>, Reply),
    Shutdown(Reply),
}
#[derive(Clone)]
pub struct Pipeline {
    commands: mpsc::Sender<Command>,
    owner: Arc<Mutex<Option<JoinHandle<()>>>>,
}
struct Run {
    shutdown: CancellationToken,
    configure: mpsc::Sender<Configure>,
    task: JoinHandle<Result<PipelineExit>>,
}
enum State {
    Idle,
    Starting {
        run: Run,
        ready: oneshot::Receiver<Result<()>>,
        reply: Option<Reply>,
    },
    Running(Run),
    Stopping {
        run: Run,
        replies: Vec<Reply>,
        shutdown: bool,
        deadline: Option<tokio::time::Instant>,
        timed_out: bool,
    },
    Completed,
    Failed,
}
enum Event {
    Ready(Result<()>),
    Finished(Result<PipelineExit>),
    TimedOut,
}
impl State {
    async fn event(&mut self) -> Event {
        match self {
            Self::Starting { run, ready, .. } => tokio::select! {
                result = ready => Event::Ready(result.context("processing startup ended unexpectedly").and_then(|result|result)),
                result = &mut run.task => Event::Finished(result.context("pipeline monitor panicked").and_then(|result|result)),
            },
            Self::Stopping { run, deadline, .. } => tokio::select! {
                result = &mut run.task => Event::Finished(result.context("pipeline monitor panicked").and_then(|result| result)),
                () = async { match deadline { Some(deadline) => tokio::time::sleep_until(*deadline).await, None => std::future::pending().await } } => Event::TimedOut,
            },
            Self::Running(run) => Event::Finished(
                (&mut run.task)
                    .await
                    .context("pipeline monitor panicked")
                    .and_then(|result| result),
            ),
            _ => std::future::pending().await,
        }
    }
}
pub(super) fn idle() -> PipelineStatus {
    PipelineStatus {
        phase: PipelinePhase::Idle,
        applied_revision: None,
        frames_processed: 0,
        error: None,
    }
}
impl Pipeline {
    pub fn spawn(config: PipelineConfig) -> Result<(Self, watch::Receiver<PipelineStatus>)> {
        Self::spawn_with_preview(config, None)
    }
    pub(crate) fn with_preview(
        config: PipelineConfig,
        preview: crate::preview::Publisher,
    ) -> Result<(Self, watch::Receiver<PipelineStatus>)> {
        Self::spawn_with_preview(config, Some(preview))
    }
    fn spawn_with_preview(
        config: PipelineConfig,
        preview: Option<crate::preview::Publisher>,
    ) -> Result<(Self, watch::Receiver<PipelineStatus>)> {
        let input = InputSource::new(config.input.clone())?;
        let (status, receiver) = watch::channel(idle());
        let (commands, incoming) = mpsc::channel(32);
        let task = tokio::spawn(own(config, input, preview, status, incoming));
        Ok((
            Self {
                commands,
                owner: Arc::new(Mutex::new(Some(task))),
            },
            receiver,
        ))
    }
    pub async fn start(&self) -> Result<()> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Start(reply))
            .await
            .context("pipeline unavailable")?;
        result.await.context("pipeline startup reply unavailable")?
    }
    pub async fn stop(&self) -> Result<()> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Stop(reply))
            .await
            .context("pipeline unavailable")?;
        result.await.context("pipeline stop reply unavailable")?
    }
    pub async fn configure(&self, config: PipelineConfig) -> Result<()> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Configure(Box::new(config), reply))
            .await
            .context("pipeline unavailable")?;
        result
            .await
            .context("pipeline configuration reply unavailable")?
    }
    pub async fn shutdown(&self) -> Result<()> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Shutdown(reply))
            .await
            .context("pipeline unavailable")?;
        let stopped = result
            .await
            .context("pipeline shutdown reply unavailable")?;
        let owner = self
            .owner
            .lock()
            .map_err(|_| anyhow!("pipeline owner lock poisoned"))?
            .take();
        if let Some(owner) = owner {
            owner.await.context("pipeline owner panicked")?;
        }
        stopped
    }
}
fn fail(status: &watch::Sender<PipelineStatus>, error: &anyhow::Error) {
    let message = format!("{error:#}");
    if status.borrow().error.as_deref() != Some(message.as_str()) {
        tracing::error!(error = %message, "pipeline failed");
    }
    status.send_modify(|status| {
        status.phase = PipelinePhase::Failed;
        status.error = Some(message);
    });
}
async fn configure_run(
    run: Option<&Run>,
    previous: &PipelineConfig,
    config: &PipelineConfig,
) -> Result<()> {
    if let Some(run) = run {
        if previous.requires_restart(config) {
            bail!("stop the pipeline before changing input or output");
        }
        let (reply, result) = oneshot::channel();
        run.configure
            .send(Configure {
                settings: config.processing,
                revision: config.revision,
                reply,
            })
            .await
            .context("processor unavailable while applying settings")?;
        result
            .await
            .context("processor ended while applying settings")??;
    }
    Ok(())
}
#[allow(
    clippy::too_many_lines,
    reason = "keep lifecycle transitions and their owned resources in one exhaustive event loop"
)]
async fn own(
    mut config: PipelineConfig,
    mut input: InputSource,
    preview: Option<crate::preview::Publisher>,
    status: watch::Sender<PipelineStatus>,
    mut commands: mpsc::Receiver<Command>,
) {
    let mut state = State::Idle;
    let mut accepting_commands = true;
    loop {
        tokio::select! {
            command = commands.recv(), if accepting_commands => {
                if command.is_none() { accepting_commands = false; }
                match command {
                    Some(Command::Start(reply)) => {
                        match state {
                            State::Running(_) => { let _ = reply.send(Ok(())); },
                            State::Starting { .. } | State::Stopping { .. } => { let _ = reply.send(Err(anyhow!("pipeline lifecycle change already running"))); },
                            _ => {
                                status.send_replace(PipelineStatus { phase:PipelinePhase::Starting,..idle() });
                                match worker::launch(config.clone(),status.clone(),preview.clone(),input.clone()) {
                                    Ok((session,ready)) => {
                                        let run = Run {shutdown:session.shutdown.clone(),configure:session.configure.clone(),task:tokio::spawn(session.wait())};
                                        state = State::Starting {run,ready,reply:Some(reply)};
                                    },
                                    Err(error) => { fail(&status,&error); state=State::Failed; let _ = reply.send(Err(error)); },
                                }
                            }
                        }
                    },
                    Some(Command::Configure(next, reply)) => {
                        let next=*next;
                        let run = match &state { State::Running(run) => Some(run), State::Starting { .. } | State::Stopping { .. } => {
                            let _ = reply.send(Err(anyhow!("pipeline lifecycle change already running")));
                            continue;
                        }, _=>None };
                        let result = async {
                            configure_run(run,&config,&next).await?;
                            let next_input = input.reconfigure(next.input.clone()).await?;
                            input = next_input;
                            config = next;
                            Ok(())
                        }.await;
                        let _ = reply.send(result);
                    },
                    command => {
                        let (reply,shutting_down) = match command {
                            Some(Command::Stop(reply)) => (Some(reply),false),
                            Some(Command::Shutdown(reply)) => (Some(reply),true),
                            None => (None,true),
                            _ => unreachable!(),
                        };
                        let previous = std::mem::replace(&mut state,State::Idle);
                        match previous {
                            State::Starting {run,reply:start,..} => {
                                run.shutdown.cancel();
                                if let Some(start)=start { let _ = start.send(Err(anyhow!("pipeline startup cancelled"))); }
                                tracing::info!("pipeline stopping");
                                status.send_modify(|status|status.phase=PipelinePhase::Stopping);
                                state=State::Stopping {run,replies:reply.into_iter().collect(),shutdown:shutting_down,deadline:Some(tokio::time::Instant::now()+std::time::Duration::from_secs(5)),timed_out:false};
                            },
                            State::Running(run) => {
                                run.shutdown.cancel();
                                tracing::info!("pipeline stopping");
                                status.send_modify(|status|status.phase=PipelinePhase::Stopping);
                                state=State::Stopping {run,replies:reply.into_iter().collect(),shutdown:shutting_down,deadline:Some(tokio::time::Instant::now()+std::time::Duration::from_secs(5)),timed_out:false};
                            },
                            State::Stopping {run,mut replies,shutdown,deadline,timed_out} => {
                                if timed_out {
                                    if let Some(reply)=reply { let _=reply.send(Err(anyhow!("pipeline cleanup timed out; resources are still stopping"))); }
                                } else if replies.len() < 2 { replies.extend(reply); }
                                else if let Some(reply) = reply { let _=reply.send(Err(anyhow!("pipeline stop already pending"))); }
                                state=State::Stopping {run,replies,shutdown:shutdown||shutting_down,deadline,timed_out};
                            },
                            _ => {
                                if shutting_down {
                                    let result=input.shutdown().await;
                                    if let Some(reply)=reply { let _=reply.send(result); }
                                    return;
                                }
                                status.send_replace(idle());
                                if let Some(reply)=reply { let _=reply.send(Ok(())); }
                            },
                        }
                    }
                }
            },
            event = state.event() => {
                let previous = std::mem::replace(&mut state,State::Idle);
                match (previous,event) {
                    (State::Starting {run,mut reply,..}, Event::Ready(Ok(()))) => {
                        tracing::info!("pipeline ready; waiting for input frames");
                        if let Some(reply)=reply.take() { let _=reply.send(Ok(())); }
                        state=State::Running(run);
                    },
                    (State::Starting {run,reply,..}, Event::Ready(Err(error))) => {
                        run.shutdown.cancel();
                        fail(&status,&error);
                        // Startup failure is acknowledged only after resource cleanup and join.
                        state=State::Stopping {run,replies:reply.into_iter().collect(),shutdown:false,deadline:Some(tokio::time::Instant::now()+std::time::Duration::from_secs(5)),timed_out:false};
                    },
                    (State::Stopping {run,replies,shutdown,..}, Event::TimedOut) => {
                        let error=anyhow!("pipeline cleanup timed out; resources are still stopping");
                        fail(&status,&error);
                        for reply in replies { let _=reply.send(Err(anyhow!("{error:#}"))); }
                        state=State::Stopping {run,replies:Vec::new(),shutdown,deadline:None,timed_out:true};
                    },
                    (previous,Event::Finished(mut result)) => {
                        if matches!(&previous,State::Stopping {timed_out:true,..}) && result.is_ok() {
                            result=Err(anyhow!("pipeline cleanup exceeded its deadline; resources have now stopped"));
                        }
                        tracing::info!(outcome = ?result.as_ref().ok(), frames_processed = status.borrow().frames_processed, "pipeline workers stopped");
                        let (replies,shutting_down)=match previous {
                            State::Starting {reply,..}=>(reply.into_iter().collect(),false),
                            State::Stopping {replies,shutdown,..}=>(replies,shutdown),
                            _=>(Vec::new(),false),
                        };
                        match &result {
                            Ok(PipelineExit::Completed)=>status.send_modify(|status|status.phase=PipelinePhase::Completed),
                            Ok(PipelineExit::Stopped)=>status.send_modify(|status|status.phase=PipelinePhase::Idle),
                            Err(error)=>fail(&status,error),
                        }
                        state=if result.is_err() {State::Failed} else if matches!(result,Ok(PipelineExit::Completed)) {State::Completed} else {State::Idle};
                        let cleanup=if shutting_down {input.shutdown().await} else {Ok(())};
                        let error=result.err().or_else(||cleanup.err());
                        for reply in replies {
                            let _=reply.send(match &error {Some(error)=>Err(anyhow!("{error:#}")),None=>Ok(())});
                        }
                        if shutting_down { return; }
                    },
                    _=>unreachable!("event belongs to the current lifecycle state"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_contract::{ConfigRevision, Output};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    async fn message(tv: &mut tokio::net::TcpStream) -> serde_json::Value {
        let mut prefix = [0; 4];
        tv.read_exact(&mut prefix).await.unwrap();
        let mut bytes = vec![0; u32::from_be_bytes(prefix) as usize];
        tv.read_exact(&mut bytes).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }
    fn config() -> PipelineConfig {
        let mut config: prysm_contract::Config =
            toml::from_str(include_str!("../../../../prysm.toml")).unwrap();
        let Output::Wled { url, .. } = &mut config.output;
        *url = "http://127.0.0.1/".parse().unwrap();
        PipelineConfig::from(&config)
    }

    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "exercise all six live settings and listener/session lifetime through the same TV connection"
    )]
    async fn listener_survives_stop_live_config_does_not_reconnect_and_shutdown_joins() {
        let mut config = config();
        config.input = prysm_contract::Input::Webos {
            listen: "127.0.0.1:0".parse().unwrap(),
            capture_resolution: [4, 2].try_into().unwrap(),
            refresh_rate_hz: 30.try_into().unwrap(),
        };
        // Reserve a concrete fixture address for the application-scoped listener.
        let available = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = available.local_addr().unwrap();
        drop(available);
        let prysm_contract::Input::Webos { listen, .. } = &mut config.input else {
            unreachable!()
        };
        *listen = address;
        let (pipeline, mut status) = Pipeline::spawn(config.clone()).unwrap();
        for index in 0..2 {
            let mut tv = tokio::net::TcpStream::connect(address).await.unwrap();
            tv.write_all(&prysm_webos_protocol::HELLO.encode().unwrap())
                .await
                .unwrap();
            assert_eq!(message(&mut tv).await["type"], "idle");
            pipeline.start().await.unwrap();
            assert_eq!(message(&mut tv).await["type"], "capture_settings");
            let mut frame = prysm_webos_protocol::Header {
                width: 4,
                height: 2,
                format: prysm_webos_protocol::PixelFormat::Rgb,
                codec: prysm_webos_protocol::Codec::Raw,
                payload_len: 24,
            }
            .encode()
            .unwrap()
            .to_vec();
            frame.extend([255; 24]);
            tv.write_all(&frame).await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while status.borrow().frames_processed < 1 {
                    status.changed().await.unwrap();
                }
            })
            .await
            .unwrap();
            for setting in 0..6 {
                let before = status.borrow().frames_processed;
                config.revision = ConfigRevision::new(index * 6 + setting + 1);
                match setting {
                    0 => {
                        config.processing.brightness_percent =
                            if index == 0 { 50 } else { 60 }.try_into().unwrap();
                    }
                    1 => {
                        config.processing.shadow_lift_percent =
                            if index == 0 { 25 } else { 35 }.try_into().unwrap();
                    }
                    2 => {
                        config.processing.smoothing_seconds =
                            if index == 0 { 0.0 } else { 0.2 }.try_into().unwrap();
                    }
                    3 => {
                        config.processing.sample_density =
                            if index == 0 { 32 } else { 64 }.try_into().unwrap();
                    }
                    4 => {
                        config.processing.spatial_reach_percent =
                            if index == 0 { 40 } else { 60 }.try_into().unwrap();
                    }
                    5 => config.processing.remove_black_bars = !config.processing.remove_black_bars,
                    _ => unreachable!(),
                }
                pipeline.configure(config.clone()).await.unwrap();
                assert_eq!(status.borrow().applied_revision, Some(config.revision));
                assert_eq!(
                    status.borrow().frames_processed,
                    before,
                    "live setting {setting} must not reset processing history/session"
                );
                tv.write_all(&frame).await.unwrap();
                tokio::time::timeout(std::time::Duration::from_secs(2), async {
                    while status.borrow().frames_processed == before {
                        status.changed().await.unwrap();
                    }
                })
                .await
                .unwrap();
            }
            pipeline.stop().await.unwrap();
            assert_eq!(tv.read(&mut [0]).await.unwrap(), 0);
        }
        pipeline.shutdown().await.unwrap();
        let listener = std::net::TcpListener::bind(address)
            .expect("shutdown must release listener before acknowledgment");
        drop(listener);
    }
    #[tokio::test]
    async fn startup_failure_is_joined_and_http_owner_can_accept_next_command() {
        let mut config = config();
        config.input = prysm_contract::Input::File {
            path: "/missing/prysm-startup.mp4".to_owned().try_into().unwrap(),
            capture_resolution: [4, 2].try_into().unwrap(),
            refresh_rate_hz: 30.try_into().unwrap(),
        };
        let (pipeline, status) = Pipeline::spawn(config.clone()).unwrap();
        let error = pipeline.start().await.unwrap_err();
        assert!(format!("{error:#}").contains("Cannot open video"));
        assert_eq!(status.borrow().phase, PipelinePhase::Failed);
        pipeline.configure(config).await.unwrap();
        pipeline.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn stop_deadline_keeps_the_active_join_owned() {
        let (release, gate) = oneshot::channel();
        let run = Run {
            shutdown: CancellationToken::new(),
            configure: mpsc::channel(8).0,
            task: tokio::spawn(async {
                gate.await.unwrap();
                Ok(PipelineExit::Stopped)
            }),
        };
        let mut state = State::Stopping {
            run,
            replies: Vec::new(),
            shutdown: false,
            deadline: Some(tokio::time::Instant::now()),
            timed_out: false,
        };
        assert!(matches!(state.event().await, Event::TimedOut));
        let State::Stopping { run, .. } = state else {
            panic!("timeout must retain active resources")
        };
        assert!(!run.task.is_finished());
        release.send(()).unwrap();
        assert_eq!(run.task.await.unwrap().unwrap(), PipelineExit::Stopped);
    }
}
