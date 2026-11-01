use crate::actors::RenderFrame;
use crate::actors::Stop;
use crate::actors::pipeline::Completion;
use actix::{
    Actor, ActorContext, Context, Handler, Message, Recipient, StreamHandler, prelude::SendError,
};
use anyhow::anyhow;
use prysm_capture::Frame;
use prysm_contract::{ConfigRevision, PipelinePhase, PipelineStatus, ProcessingSettings};
use prysm_metrics::{FrameStage, PipelineEvent, Stage};
use prysm_processor::PrysmProcessor;
use tokio::sync::watch;

pub struct Processor {
    frame_processor: PrysmProcessor,
    renderer: Recipient<RenderFrame>,
    completion: Completion,
    status: watch::Sender<PipelineStatus>,
    availability: Option<watch::Receiver<bool>>,
}

/// Frames are best effort: `try_send` drops incoming frames when the actor is busy.
#[derive(Message)]
#[rtype(result = "()")]
pub struct FrameUpdate(pub Frame);

#[derive(Message)]
#[rtype(result = "()")]
pub struct ConfigureProcessor(pub ProcessingSettings, pub ConfigRevision);

impl Processor {
    #[must_use]
    pub fn new(
        settings: &ProcessingSettings,
        renderer: Recipient<RenderFrame>,
        completion: Completion,
        status: watch::Sender<PipelineStatus>,
        availability: Option<watch::Receiver<bool>>,
    ) -> Self {
        Self {
            frame_processor: PrysmProcessor::new(&settings.to_core()),
            renderer,
            completion,
            status,
            availability,
        }
    }

    fn update_availability(&self) {
        let available = self
            .availability
            .as_ref()
            .is_none_or(|receiver| *receiver.borrow());
        self.status.send_modify(|status| {
            if available {
                if status.frames_processed > 0 {
                    status.phase = PipelinePhase::Running;
                }
                status.error = None;
            } else {
                status.phase = PipelinePhase::Starting;
                status.error = Some("input unavailable; waiting for frames".into());
            }
        });
    }
}

impl StreamHandler<bool> for Processor {
    fn handle(&mut self, _: bool, _: &mut Self::Context) {
        // Read the latest value so queued frames or notifications cannot revive a disconnected input.
        self.update_availability();
    }

    // The frame stream, not this observation channel, owns session completion.
    fn finished(&mut self, _: &mut Self::Context) {}
}

impl Actor for Processor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.set_mailbox_capacity(1);
        if let Some(availability) = self.availability.clone() {
            self.update_availability();
            Self::add_stream(
                futures::stream::unfold(availability, |mut receiver| async {
                    receiver.changed().await.ok()?;
                    let available = *receiver.borrow_and_update();
                    Some((available, receiver))
                }),
                ctx,
            );
        }
    }
}

impl Handler<FrameUpdate> for Processor {
    type Result = ();

    fn handle(&mut self, msg: FrameUpdate, ctx: &mut Self::Context) {
        let process_time = prysm_metrics::timer(Stage::FrameProcess);
        let colors = self.frame_processor.process_frame(msg.0.clone());
        drop(process_time);
        prysm_metrics::frame(FrameStage::Processed, msg.0.format().into());
        let rendered = RenderFrame {
            frame: msg.0,
            viewport: self.frame_processor.viewport(),
            colors,
        };
        match self.renderer.try_send(rendered) {
            Ok(()) => {}
            Err(SendError::Full(_)) => prysm_metrics::event(PipelineEvent::RendererMailboxFull),
            Err(SendError::Closed(_)) => {
                let _ = self
                    .completion
                    .try_send(Err(anyhow!("renderer actor unavailable")));
                ctx.stop();
                return;
            }
        }
        self.status.send_modify(|status| {
            status.frames_processed = status.frames_processed.saturating_add(1);
        });
        self.update_availability();
    }
}

impl Handler<Stop> for Processor {
    type Result = ();

    fn handle(&mut self, _: Stop, ctx: &mut Self::Context) {
        ctx.stop();
    }
}

impl Handler<ConfigureProcessor> for Processor {
    type Result = ();

    fn handle(&mut self, msg: ConfigureProcessor, _: &mut Self::Context) {
        self.frame_processor.configure(&msg.0.to_core());
        self.status
            .send_modify(|status| status.applied_revision = Some(msg.1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix::AsyncContext;
    use prysm_capture::PixelFormat;
    use prysm_core::{EdgeColors, LinearColor};
    use std::{cell::RefCell, rc::Rc};

    struct RecordingRenderer {
        colors: Rc<RefCell<Vec<EdgeColors>>>,
    }

    impl Actor for RecordingRenderer {
        type Context = Context<Self>;
    }

    impl Handler<RenderFrame> for RecordingRenderer {
        type Result = ();

        fn handle(&mut self, msg: RenderFrame, _: &mut Context<Self>) -> Self::Result {
            self.colors.borrow_mut().push(msg.colors);
        }
    }

    #[derive(Message)]
    #[rtype(result = "()")]
    struct Flush;

    impl Handler<Flush> for RecordingRenderer {
        type Result = ();
        fn handle(&mut self, _: Flush, _: &mut Context<Self>) {}
    }

    fn status() -> watch::Sender<PipelineStatus> {
        watch::channel(super::super::runtime::idle()).0
    }

    fn completion() -> Completion {
        tokio::sync::mpsc::channel(3).0
    }

    fn settings() -> ProcessingSettings {
        ProcessingSettings {
            smoothing_seconds: 0.0.try_into().unwrap(),
            remove_black_bars: false,
            ..ProcessingSettings::default()
        }
    }

    fn white_frame() -> FrameUpdate {
        FrameUpdate(Frame::fill(255, 4, 4, PixelFormat::RGB24).unwrap())
    }

    #[actix::test]
    async fn queued_frame_cannot_report_a_disconnected_input_as_running() {
        let output = RecordingRenderer {
            colors: Rc::new(RefCell::new(Vec::new())),
        }
        .start();
        let status = status();
        let (availability, receiver) = watch::channel(true);
        let processor = Processor::new(
            &settings(),
            output.recipient(),
            completion(),
            status.clone(),
            Some(receiver),
        )
        .start();
        processor.send(white_frame()).await.unwrap();
        assert_eq!(status.borrow().phase, PipelinePhase::Running);
        availability.send_replace(false);
        processor.send(white_frame()).await.unwrap();
        assert_eq!(status.borrow().frames_processed, 2);
        assert_eq!(status.borrow().phase, PipelinePhase::Starting);
        assert!(status.borrow().error.is_some());
        availability.send_replace(true);
        processor.send(white_frame()).await.unwrap();
        assert_eq!(status.borrow().phase, PipelinePhase::Running);
        assert!(status.borrow().error.is_none());
    }

    #[actix::test]
    async fn forwards_colors_and_applies_settings_between_frames() {
        let colors = Rc::new(RefCell::new(Vec::new()));
        let output = RecordingRenderer {
            colors: colors.clone(),
        }
        .start();
        let mut settings = settings();
        let processor = Processor::new(
            &settings,
            output.clone().recipient(),
            completion(),
            status(),
            None,
        )
        .start();

        processor.send(white_frame()).await.unwrap();
        output.send(Flush).await.unwrap();
        settings.brightness_percent = 0.try_into().unwrap();
        processor
            .send(ConfigureProcessor(settings, ConfigRevision::new(1)))
            .await
            .unwrap();
        processor.send(white_frame()).await.unwrap();
        output.send(Flush).await.unwrap();

        let colors = colors.borrow();
        assert_eq!(colors.len(), 2);
        for strip in [
            &colors[0].top,
            &colors[0].right,
            &colors[0].bottom,
            &colors[0].left,
        ] {
            assert_eq!(strip.sample_at(0.5), LinearColor::new(1.0, 1.0, 1.0));
        }
        for strip in [
            &colors[1].top,
            &colors[1].right,
            &colors[1].bottom,
            &colors[1].left,
        ] {
            assert_eq!(strip.sample_at(0.5), LinearColor::black());
        }
    }

    #[actix::test]
    async fn drops_colors_when_renderer_mailbox_is_full() {
        let colors = Rc::new(RefCell::new(Vec::new()));
        let mut context = Context::<RecordingRenderer>::new();
        context.set_mailbox_capacity(1);
        let output = context.address();
        let processor = Processor::new(
            &settings(),
            output.clone().recipient(),
            completion(),
            status(),
            None,
        )
        .start();

        processor.send(white_frame()).await.unwrap();
        processor.send(white_frame()).await.unwrap();
        context.run(RecordingRenderer {
            colors: colors.clone(),
        });
        output.send(Flush).await.unwrap();
        assert_eq!(colors.borrow().len(), 1);
    }

    #[actix::test]
    async fn reports_closed_renderer_mailbox() {
        let context = Context::<RecordingRenderer>::new();
        let output = context.address().recipient();
        drop(context);
        let (completion, mut outcomes) = tokio::sync::mpsc::channel(3);
        let processor = Processor::new(&settings(), output, completion, status(), None).start();

        processor.send(white_frame()).await.unwrap();
        let error = outcomes.recv().await.unwrap().unwrap_err();
        assert_eq!(error.to_string(), "renderer actor unavailable");
    }
}
