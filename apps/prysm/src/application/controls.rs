//! Native controls are a REST client, with networking kept off the UI thread.
mod worker;

use prysm_contract::{
    ApiError, ErrorCode, OperationResponse, OperationStatus, PipelinePhase, Source, StateResponse,
    Target, WebConfig,
};
use prysm_renderer_desktop::egui;
use std::{net::SocketAddr, time::Duration};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

type TargetRequest = (Target, oneshot::Sender<Result<OperationResponse, ApiError>>);

pub(super) struct Controls {
    state: watch::Receiver<Option<Result<StateResponse, ApiError>>>,
    targets: mpsc::Sender<TargetRequest>,
    request: Option<oneshot::Receiver<Result<OperationResponse, ApiError>>>,
    error: Option<ApiError>,
    has_frames: bool,
}

impl Controls {
    pub(super) fn new(
        web: &WebConfig,
        shutdown: CancellationToken,
    ) -> anyhow::Result<(Self, impl FnOnce(SocketAddr) + Send + 'static)> {
        let mut headers = reqwest::header::HeaderMap::new();
        if !web.allowed_hosts.iter().any(|host| host == "*") {
            let host = web
                .allowed_hosts
                .first()
                .ok_or_else(|| anyhow::anyhow!("no allowed controller host"))?;
            headers.insert(reqwest::header::HOST, host.parse()?);
        }
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(3))
            .default_headers(headers)
            .build()?;
        let (state_tx, state) = watch::channel(None);
        let (targets, target_rx) = mpsc::channel(1);
        let controls = Self {
            state,
            targets,
            request: None,
            error: None,
            has_frames: false,
        };
        Ok((controls, move |address| {
            tokio::spawn(worker::run(http, address, state_tx, target_rx, shutdown));
        }))
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| self.show_contents(ui));
    }

    fn poll_request(&mut self) {
        if let Some(request) = self.request.as_mut() {
            match request.try_recv() {
                Ok(result) => {
                    self.request = None;
                    self.error = result.err();
                }
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.request = None;
                    self.error = Some(ApiError {
                        code: ErrorCode::Unavailable,
                        message: "Controller unavailable".into(),
                    });
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
            }
        }
    }

    fn request_target(&mut self, power: bool) {
        let (reply, request) = oneshot::channel();
        match self.targets.try_send((
            Target {
                power,
                source: Source::Prysm,
            },
            reply,
        )) {
            Ok(()) => {
                self.request = Some(request);
                self.error = None;
            }
            Err(_) => {
                self.error = Some(ApiError {
                    code: ErrorCode::Unavailable,
                    message: "Could not send capture request".into(),
                });
            }
        }
    }

    fn show_contents(&mut self, ui: &mut egui::Ui) {
        self.poll_request();
        let snapshot = self.state.borrow_and_update().clone();
        let state = match snapshot {
            Some(Ok(state)) => state,
            Some(Err(error)) => {
                ui.label("Controller unavailable");
                ui.colored_label(ui.visuals().error_fg_color, error.message);
                return;
            }
            None => {
                ui.label("Starting controller…");
                return;
            }
        };
        self.has_frames |= state.pipeline.frames_processed > 0;
        let running = matches!(
            state.pipeline.phase,
            PipelinePhase::Starting | PipelinePhase::Running
        );
        let busy =
            self.request.is_some() || matches!(state.operation, OperationStatus::Pending { .. });
        let connected = !self.targets.is_closed();
        let status = if !connected {
            "Controller unavailable"
        } else if state.restart_required {
            "Restart Prysm to apply output settings"
        } else if busy {
            "Applying…"
        } else {
            match state.pipeline.phase {
                PipelinePhase::Idle => "Stopped",
                PipelinePhase::Starting => "Starting…",
                PipelinePhase::Running => "Running",
                PipelinePhase::Completed => "Playback complete",
                PipelinePhase::Failed => "Capture failed",
            }
        };
        let status_color = if !connected
            || state.pipeline.phase == PipelinePhase::Failed
            || self.error.is_some()
            || matches!(state.operation, OperationStatus::Failed { .. })
        {
            ui.visuals().error_fg_color
        } else if busy || state.restart_required {
            ui.visuals().warn_fg_color
        } else if running {
            egui::Color32::from_rgb(92, 184, 120)
        } else {
            ui.visuals().weak_text_color()
        };
        let enabled = connected && !busy && !state.restart_required;
        let shortcut = !ui.ctx().egui_wants_keyboard_input()
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Space));
        ui.horizontal(|ui| {
            let response = ui
                .add_enabled(
                    enabled,
                    egui::Button::new(if running {
                        "Stop capture"
                    } else {
                        "Start capture"
                    })
                    .min_size(egui::vec2(0.0, 24.0))
                    .corner_radius(4),
                )
                .on_hover_text(if running {
                    "Stop capture (Space)"
                } else {
                    "Start capture (Space)"
                });
            if enabled && (response.clicked() || shortcut) {
                self.request_target(!running);
            }
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);
            if busy {
                ui.add(egui::Spinner::new().size(12.0));
            } else {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(10.0, 16.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 3.0, status_color);
            }
            ui.label(status);
        });
        let operation_error = match &state.operation {
            OperationStatus::Failed { error, .. } => Some(error.message.as_str()),
            _ => None,
        };
        if let Some(error) = self
            .error
            .as_ref()
            .map(|error| error.message.as_str())
            .or(operation_error)
            .or(state.pipeline.error.as_deref())
        {
            ui.add_space(6.0);
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        self.show_empty_state(ui, &state, enabled);
    }

    fn show_empty_state(&self, ui: &mut egui::Ui, state: &StateResponse, enabled: bool) {
        if enabled
            && !self.has_frames
            && state.pipeline.phase == PipelinePhase::Idle
            && self.error.is_none()
        {
            egui::Area::new(egui::Id::unique("capture_empty_state"))
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .order(egui::Order::Foreground)
                .interactable(false)
                .show(ui.ctx(), |ui| {
                    ui.set_width(300.0);
                    ui.vertical_centered(|ui| {
                        ui.heading("Ready to capture");
                        ui.add_space(6.0);
                        ui.label("Click Start capture or press Space to begin.");
                    });
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix::{Actor, Context, Handler};
    use axum::response::IntoResponse as _;
    use prysm_api::{ApiFailure, App, SetConfig, SetTarget};
    use prysm_contract::{
        Config, ConfigRevision, OperationId, OperationResponse, PipelineStatus, StateResponse,
    };

    struct Controller(tokio::sync::mpsc::UnboundedSender<Target>);
    impl Actor for Controller {
        type Context = Context<Self>;
    }
    impl Handler<SetTarget> for Controller {
        type Result = Result<OperationResponse, ApiFailure>;
        fn handle(&mut self, message: SetTarget, _: &mut Self::Context) -> Self::Result {
            self.0.send(message.0).unwrap();
            if message.0.power {
                Err(ApiFailure::conflict("Capture could not start"))
            } else {
                Ok(OperationResponse {
                    operation_id: OperationId::new(),
                })
            }
        }
    }
    impl Handler<SetConfig> for Controller {
        type Result = Result<Config, ApiFailure>;
        fn handle(&mut self, _: SetConfig, _: &mut Self::Context) -> Self::Result {
            unreachable!()
        }
    }

    fn draw(
        controls: &mut Controls,
        context: &egui::Context,
        events: Vec<egui::Event>,
    ) -> Vec<(String, egui::Pos2)> {
        let mut output = context.run_ui(
            egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 360.0),
                )),
                ..Default::default()
            },
            |ui| {
                egui::Panel::top("controls").show(ui, |ui| controls.show(ui));
            },
        );
        output.textures_delta.clear();
        output
            .shapes
            .into_iter()
            .filter_map(|shape| match shape.shape {
                egui::Shape::Text(text) => Some((
                    text.galley.text().to_owned(),
                    text.pos + text.galley.size() / 2.0,
                )),
                _ => None,
            })
            .collect()
    }

    fn click(controls: &mut Controls, context: &egui::Context, label: &str) {
        let labels = draw(controls, context, vec![]);
        let position = labels.iter().find(|(text, _)| text == label).unwrap().1;
        for pressed in [true, false] {
            draw(
                controls,
                context,
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::default(),
                    },
                ],
            );
        }
        draw(controls, context, vec![]);
    }

    async fn refresh(controls: &mut Controls, matches: impl Fn(&StateResponse) -> bool) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if controls
                    .state
                    .borrow_and_update()
                    .as_ref()
                    .is_some_and(|state| state.as_ref().is_ok_and(&matches))
                {
                    break;
                }
                controls.state.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
    }

    #[actix::test]
    async fn buttons_dispatch_targets_follow_external_state_and_show_failures() {
        let (targets, mut target_rx) = tokio::sync::mpsc::unbounded_channel();
        let controller = Controller(targets).start();
        let config: Config = toml::from_str(include_str!("../../../../prysm-egui.toml")).unwrap();
        let mut web = config.web.clone();
        web.allowed_hosts = vec!["controller.test:8787".into()];
        let (_config_tx, config) = tokio::sync::watch::channel(config);
        let (state_tx, state) = tokio::sync::watch::channel(StateResponse {
            source: Source::Prysm,
            observed: None,
            observation_error: None,
            operation: OperationStatus::Idle,
            wled_url: String::new(),
            config_revision: ConfigRevision::new(0),
            applied_revision: None,
            pipeline: PipelineStatus {
                applied_revision: None,
                phase: PipelinePhase::Idle,
                frames_processed: 0,
                error: None,
            },
            restart_required: false,
        });
        let shutdown = CancellationToken::new();
        let _cleanup = shutdown.clone().drop_guard();
        let (mut controls, connect) = Controls::new(&web, shutdown.clone()).unwrap();
        let context = egui::Context::default();
        assert!(
            draw(&mut controls, &context, vec![])
                .iter()
                .any(|(text, _)| text == "Starting controller…")
        );
        let unavailable = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fail_requests = unavailable.clone();
        let router = prysm_api::router()
            .split_for_parts()
            .0
            .with_state(App {
                set_target: controller.clone().recipient(),
                set_config: controller.recipient(),
                config,
                state,
            })
            .layer(axum::middleware::from_fn(
                move |request: axum::extract::Request, next: axum::middleware::Next| {
                    let fail_requests = fail_requests.clone();
                    async move {
                        assert_eq!(
                            request.headers()[reqwest::header::HOST],
                            "controller.test:8787"
                        );
                        if fail_requests.load(std::sync::atomic::Ordering::Relaxed) {
                            (
                                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                                axum::Json(ApiError {
                                    code: ErrorCode::Unavailable,
                                    message: "Controller temporarily unavailable".into(),
                                }),
                            )
                                .into_response()
                        } else {
                            next.run(request).await
                        }
                    }
                },
            ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        connect(listener.local_addr().unwrap());
        let server_shutdown = shutdown.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(server_shutdown.cancelled_owned())
                .await
                .unwrap();
        });
        refresh(&mut controls, |_| true).await;

        draw(&mut controls, &context, vec![]);
        let labels = draw(&mut controls, &context, vec![]);
        let start = labels
            .iter()
            .find(|(text, _)| text == "Start capture")
            .unwrap()
            .1;
        let status = labels.iter().find(|(text, _)| text == "Stopped").unwrap().1;
        assert!(
            status.x > start.x + 40.0 && status.x < start.x + 200.0,
            "status stays beside the action with a clear gap"
        );
        assert!(labels.iter().any(|(text, _)| text == "Ready to capture"));
        state_tx.send_modify(|state| state.pipeline.frames_processed = 1);
        refresh(&mut controls, |state| state.pipeline.frames_processed == 1).await;
        draw(&mut controls, &context, vec![]);
        state_tx.send_modify(|state| state.pipeline.frames_processed = 0);
        refresh(&mut controls, |state| state.pipeline.frames_processed == 0).await;
        assert!(
            !draw(&mut controls, &context, vec![])
                .iter()
                .any(|(text, _)| text == "Ready to capture")
        );

        for (phase, button, power) in [
            (PipelinePhase::Idle, "Start capture", true),
            (PipelinePhase::Running, "Stop capture", false),
            (PipelinePhase::Completed, "Start capture", true),
        ] {
            state_tx.send_modify(|state| state.pipeline.phase = phase);
            refresh(&mut controls, |state| state.pipeline.phase == phase).await;
            // Native UI interactions must work without a Tokio runtime on the UI thread.
            std::thread::scope(|scope| {
                scope
                    .spawn(|| click(&mut controls, &context, button))
                    .join()
                    .unwrap();
            });
            let target = tokio::time::timeout(std::time::Duration::from_secs(1), target_rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                target,
                Target {
                    power,
                    source: Source::Prysm
                }
            );
            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                while controls.request.is_some() {
                    draw(&mut controls, &context, vec![]);
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            let labels = draw(&mut controls, &context, vec![]);
            assert_eq!(
                labels
                    .iter()
                    .any(|(text, _)| text == "Capture could not start"),
                power
            );
        }

        state_tx.send_modify(|state| {
            state.operation = OperationStatus::Pending {
                id: OperationId::new(),
            }
        });
        refresh(&mut controls, |state| {
            matches!(state.operation, OperationStatus::Pending { .. })
        })
        .await;
        click(&mut controls, &context, "Start capture");
        assert!(target_rx.try_recv().is_err());
        assert!(
            draw(&mut controls, &context, vec![])
                .iter()
                .any(|(text, _)| text == "Applying…")
        );
        state_tx.send_modify(|state| {
            state.operation = OperationStatus::Idle;
            state.restart_required = true;
        });
        refresh(&mut controls, |state| state.restart_required).await;
        click(&mut controls, &context, "Start capture");
        assert!(target_rx.try_recv().is_err());

        state_tx.send_modify(|state| state.restart_required = false);
        refresh(&mut controls, |state| !state.restart_required).await;
        draw(
            &mut controls,
            &context,
            vec![egui::Event::Key {
                key: egui::Key::Space,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        draw(&mut controls, &context, vec![]);
        let target = tokio::time::timeout(std::time::Duration::from_secs(1), target_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            target,
            Target {
                power: true,
                source: Source::Prysm
            }
        );
        unavailable.store(true, std::sync::atomic::Ordering::Relaxed);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if controls
                    .state
                    .borrow_and_update()
                    .as_ref()
                    .is_some_and(Result::is_err)
                {
                    break;
                }
                controls.state.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert!(
            draw(&mut controls, &context, vec![])
                .iter()
                .any(|(text, _)| text == "Controller unavailable")
        );
        unavailable.store(false, std::sync::atomic::Ordering::Relaxed);
        refresh(&mut controls, |_| true).await;
        assert!(
            draw(&mut controls, &context, vec![])
                .iter()
                .any(|(text, _)| text == "Start capture")
        );
        assert!(
            target_rx.try_recv().is_err(),
            "reconnection must not replay commands"
        );
        shutdown.cancel();
        server.await.unwrap();
    }
}
