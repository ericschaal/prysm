use super::backend::{CaptureAction, LightingBackend, WledCommand};
use anyhow::{Context, Result, bail, ensure};
use prysm_contract::{PipelinePhase, Source, Target};

use std::time::Duration;
use tokio::time::{Instant, sleep};
use tokio_util::sync::CancellationToken;

pub(super) async fn apply(
    backend: &impl LightingBackend,
    target: Target,
    cancel: &CancellationToken,
) -> Result<()> {
    if !target.power {
        // Stop even if WLED is offline; reapply after the last DDP packet so override takes effect.
        let preliminary = backend.wled(WledCommand::SUPPRESS_AND_OFF).await;
        let stopped = backend.capture(CaptureAction::Stop).await;
        stopped.context("cannot stop Prysm")?;
        backend.wled(WledCommand::RELEASE).await?;
        backend.wled(WledCommand::OFF).await.with_context(|| {
            format!("cannot switch WLED off (initial attempt: {preliminary:?})")
        })?;
    } else if target.source == Source::Wled {
        backend
            .capture(CaptureAction::Stop)
            .await
            .context("cannot stop Prysm")?;
        backend.wled(WledCommand::RELEASE).await?;
        backend.wled(WledCommand::EFFECTS).await?;
    } else {
        // A previous main-segment realtime session can ignore lor until live has been released.
        backend.wled(WledCommand::RELEASE).await?;
        // Finish each WLED write before cancellation so a late On cannot undo Stop.
        ensure!(!cancel.is_cancelled(), "lighting startup cancelled");
        backend.wled(WledCommand::REALTIME).await?;
        tokio::select! {
            biased;
            () = cancel.cancelled() => bail!("lighting startup cancelled"),
            result = backend.capture(CaptureAction::Start) => result.context("cannot start Prysm")?,
        }
    }
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        let observed = tokio::select! {
            biased;
            () = cancel.cancelled() => bail!("lighting startup cancelled"),
            result = backend.observe() => result?,
        };
        if observed.matches(target) {
            return Ok(());
        }
        if target.power
            && target.source == Source::Prysm
            && observed.pipeline_phase == PipelinePhase::Failed
        {
            bail!("capture failed; check the input and application logs");
        }
        if Instant::now() >= deadline {
            bail!("requested state was not confirmed; check capture and WLED");
        }
        tokio::select! {
            () = cancel.cancelled() => bail!("lighting startup cancelled"),
            () = sleep(Duration::from_millis(300)) => {},
        }
    }
}

pub(super) async fn switch(backend: &impl LightingBackend, target: Target) -> Result<()> {
    let cancel = CancellationToken::new();
    let previous = backend.observe().await;
    // A source choice while observed Off only changes what the next On request starts.
    if !target.power && previous.as_ref().is_ok_and(|state| state.matches(target)) {
        return Ok(());
    }
    if let Err(error) = apply(backend, target, &cancel).await {
        if target.power && target.source == Source::Prysm {
            backend
                .capture(CaptureAction::Stop)
                .await
                .with_context(|| format!("{error:#}; stopping automatic retries also failed"))?;
            let rollback = previous.map_or(
                Target {
                    power: false,
                    source: Source::Wled,
                },
                |state| Target {
                    power: state.power,
                    source: if state.pipeline_phase == PipelinePhase::Running
                        && state.realtime
                        && state.live_override == 0
                    {
                        Source::Prysm
                    } else {
                        Source::Wled
                    },
                },
            );
            if let Err(restore_error) = apply(backend, rollback, &cancel).await {
                let stopped = backend.capture(CaptureAction::Stop).await;
                bail!(
                    "{error:#}; restoring previous state failed: {restore_error:#}; cancel automatic retries: {stopped:?}"
                );
            }
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_contract::Observation;

    use tokio::sync::Mutex;

    #[tokio::test]
    async fn cancelled_startup_finishes_an_in_flight_wled_write_before_stop() {
        struct DelayedOn {
            backend: Fake,
            entered: tokio::sync::Notify,
            release: tokio::sync::Notify,
        }
        impl LightingBackend for DelayedOn {
            async fn capture(&self, action: CaptureAction) -> Result<()> {
                self.backend.capture(action).await
            }
            async fn wled(&self, value: WledCommand) -> Result<()> {
                if value.on == Some(true) {
                    self.entered.notify_one();
                    self.release.notified().await;
                }
                self.backend.wled(value).await
            }
            async fn observe(&self) -> Result<Observation> {
                self.backend.observe().await
            }
        }
        let backend = std::sync::Arc::new(DelayedOn {
            backend: Fake::new(false),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let cancel = CancellationToken::new();
        let pending = {
            let backend = backend.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move {
                apply(
                    &*backend,
                    Target {
                        power: true,
                        source: Source::Prysm,
                    },
                    &cancel,
                )
                .await
            })
        };
        tokio::time::timeout(Duration::from_secs(1), backend.entered.notified())
            .await
            .unwrap();
        cancel.cancel();
        tokio::task::yield_now().await;
        assert!(
            !pending.is_finished(),
            "do not cancel an accepted WLED write"
        );
        backend.release.notify_one();
        assert!(pending.await.unwrap().is_err());
        assert!(
            !backend
                .backend
                .calls
                .lock()
                .await
                .iter()
                .any(|call| call == "start")
        );
        switch(
            &*backend,
            Target {
                power: false,
                source: Source::Prysm,
            },
        )
        .await
        .unwrap();
        assert!(!backend.backend.state.lock().await.power);
    }
    struct Fake {
        state: Mutex<Observation>,
        calls: Mutex<Vec<String>>,
        fail: Option<&'static str>,
    }
    impl Fake {
        fn new(power: bool) -> Self {
            Self {
                state: Mutex::new(Observation {
                    power,
                    realtime: false,
                    realtime_source: None,
                    live_override: 2,
                    pipeline_phase: PipelinePhase::Idle,
                }),
                calls: Mutex::new(vec![]),
                fail: None,
            }
        }
    }
    impl LightingBackend for Fake {
        async fn capture(&self, action: CaptureAction) -> Result<()> {
            let action = match action {
                CaptureAction::Start => "start",
                CaptureAction::Stop => "stop",
            };
            self.calls.lock().await.push(action.into());
            if self.fail == Some(action)
                || (self.fail == Some("start-stop") && matches!(action, "start" | "stop"))
            {
                bail!("denied");
            }
            let mut state = self.state.lock().await;
            state.pipeline_phase = if action == "start" {
                PipelinePhase::Running
            } else {
                PipelinePhase::Idle
            };
            if action == "start" {
                state.realtime = true;
            }
            Ok(())
        }
        async fn wled(&self, command: WledCommand) -> Result<()> {
            let value = serde_json::to_value(command)?;
            self.calls.lock().await.push(value.to_string());
            if self.fail == Some("wled") {
                bail!("offline");
            }
            let mut state = self.state.lock().await;
            if let Some(on) = value["on"].as_bool() {
                state.power = on;
            }
            if !state.realtime
                && let Some(lor) = value["lor"].as_u64()
            {
                state.live_override = u8::try_from(lor)?;
            }
            if value["live"] == false {
                state.realtime = false;
            }
            Ok(())
        }
        async fn observe(&self) -> Result<Observation> {
            Ok(self.state.lock().await.clone())
        }
    }
    #[tokio::test]
    async fn all_transitions_and_repeated_requests() {
        let backend = Fake::new(false);
        for target in [
            Target {
                power: false,
                source: Source::Wled,
            },
            Target {
                power: true,
                source: Source::Wled,
            },
            Target {
                power: true,
                source: Source::Prysm,
            },
            Target {
                power: true,
                source: Source::Prysm,
            },
            Target {
                power: false,
                source: Source::Prysm,
            },
        ] {
            switch(&backend, target).await.unwrap();
            assert!(backend.observe().await.unwrap().matches(target));
        }
    }
    #[tokio::test]
    async fn source_selection_while_off_has_no_side_effects() {
        let backend = Fake::new(false);
        switch(
            &backend,
            Target {
                power: false,
                source: Source::Wled,
            },
        )
        .await
        .unwrap();
        assert!(backend.calls.lock().await.is_empty());
    }
    #[tokio::test]
    async fn off_stops_even_when_wled_is_offline() {
        let mut backend = Fake::new(true);
        backend.fail = Some("wled");
        assert!(
            switch(
                &backend,
                Target {
                    power: false,
                    source: Source::Prysm
                }
            )
            .await
            .is_err()
        );
        assert!(backend.calls.lock().await.iter().any(|call| call == "stop"));
    }
    #[tokio::test]
    async fn start_failure_restores_effect_and_cancels_retries() {
        let mut backend = Fake::new(true);
        backend.fail = Some("start");
        assert!(
            switch(
                &backend,
                Target {
                    power: true,
                    source: Source::Prysm
                }
            )
            .await
            .is_err()
        );
        assert!(backend.observe().await.unwrap().matches(Target {
            power: true,
            source: Source::Wled
        }));
        assert!(backend.calls.lock().await.iter().any(|call| call == "stop"));
    }
    #[tokio::test]
    async fn effects_are_never_enabled_when_stop_is_denied() {
        let mut backend = Fake::new(true);
        backend.fail = Some("stop");
        assert!(
            switch(
                &backend,
                Target {
                    power: true,
                    source: Source::Wled
                }
            )
            .await
            .is_err()
        );
        assert_eq!(*backend.calls.lock().await, ["stop"]);
    }
    #[test]
    fn rejects_unknown_and_invalid_fields() {
        for input in [
            r#"{"power":true,"source":"other"}"#,
            r#"{"power":true,"source":"prysm","command":"stop"}"#,
            r#"{"power":"yes","source":"wled"}"#,
        ] {
            assert!(serde_json::from_str::<Target>(input).is_err());
        }
    }
    #[tokio::test]
    async fn compensation_failure_is_reported() {
        let mut backend = Fake::new(true);
        backend.fail = Some("start-stop");
        let error = switch(
            &backend,
            Target {
                power: true,
                source: Source::Prysm,
            },
        )
        .await
        .unwrap_err();
        assert!(format!("{error:#}").contains("stopping automatic retries also failed"));
    }
    #[tokio::test]
    async fn effects_release_main_segment_before_setting_override() {
        let backend = Fake::new(true);
        {
            let mut state = backend.state.lock().await;
            state.realtime = true;
            state.live_override = 0;
            state.pipeline_phase = PipelinePhase::Running;
        }
        switch(
            &backend,
            Target {
                power: true,
                source: Source::Wled,
            },
        )
        .await
        .unwrap();
        assert!(backend.observe().await.unwrap().matches(Target {
            power: true,
            source: Source::Wled
        }));
        assert_eq!(backend.calls.lock().await[0], "stop");
        assert_eq!(backend.calls.lock().await[1], r#"{"live":false}"#);
    }
}
