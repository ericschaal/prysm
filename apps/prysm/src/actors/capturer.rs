use crate::actors::Stop;
use crate::actors::pipeline::{Completion, PipelineExit};
use crate::actors::processor::FrameUpdate;
use actix::{
    Actor, ActorContext, Addr, Context, Handler, Recipient, StreamHandler, prelude::SendError,
};
use anyhow::Result;
use futures::Stream;
use prysm_capture::Frame;
use prysm_metrics::{FrameStage, PipelineEvent};

/// One capture session. Its context owns and drops the input stream.
pub struct Capturer {
    processor: Recipient<FrameUpdate>,
    completion: Option<Completion>,
}

impl Capturer {
    pub fn spawn(
        frames: impl Stream<Item = Result<Frame>> + 'static,
        processor: Recipient<FrameUpdate>,
        completion: Completion,
    ) -> Addr<Self> {
        Self::create(move |ctx| {
            Self::add_stream(frames, ctx);
            Self {
                processor,
                completion: Some(completion),
            }
        })
    }

    fn finish(&mut self, result: Result<PipelineExit>, ctx: &mut Context<Self>) {
        if let Some(completion) = self.completion.take() {
            let _ = completion.try_send(result);
        }
        ctx.stop();
    }
}

impl Actor for Capturer {
    type Context = Context<Self>;

    fn stopped(&mut self, _: &mut Self::Context) {
        if let Some(completion) = self.completion.take() {
            let _ = completion.try_send(Err(anyhow::anyhow!("capture actor stopped unexpectedly")));
        }
    }
}

impl StreamHandler<Result<Frame>> for Capturer {
    fn handle(&mut self, result: Result<Frame>, ctx: &mut Self::Context) {
        let frame = match result {
            Ok(frame) => frame,
            Err(error) => {
                prysm_metrics::event(PipelineEvent::InputFailed);
                tracing::error!(error = %format!("{error:#}"), "capture session failed");
                self.finish(Err(error), ctx);
                return;
            }
        };
        prysm_metrics::frame(FrameStage::InputDelivered, frame.format().into());
        match self.processor.try_send(FrameUpdate(frame)) {
            Ok(()) => (),
            Err(SendError::Full(_)) => prysm_metrics::event(PipelineEvent::ProcessorMailboxFull),
            Err(SendError::Closed(_)) => {
                tracing::error!("processor actor unavailable");
                self.finish(Err(anyhow::anyhow!("processor actor unavailable")), ctx);
            }
        }
    }

    fn finished(&mut self, ctx: &mut Self::Context) {
        self.finish(Ok(PipelineExit::Completed), ctx);
    }
}

impl Handler<Stop> for Capturer {
    type Result = ();

    fn handle(&mut self, _: Stop, ctx: &mut Self::Context) {
        self.finish(Ok(PipelineExit::Stopped), ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix::{Addr, AsyncContext, AtomicResponse, WrapFuture};
    use futures::{StreamExt, stream};
    use std::{
        cell::Cell,
        rc::Rc,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };
    use tokio::sync::oneshot;

    fn completion() -> Completion {
        tokio::sync::mpsc::channel(3).0
    }

    struct RecordingProcessor {
        received: Rc<Cell<usize>>,
        gate: Option<oneshot::Receiver<()>>,
    }

    impl Actor for RecordingProcessor {
        type Context = Context<Self>;

        fn started(&mut self, ctx: &mut Self::Context) {
            ctx.set_mailbox_capacity(1);
        }
    }

    impl Handler<FrameUpdate> for RecordingProcessor {
        type Result = AtomicResponse<Self, ()>;

        fn handle(&mut self, _: FrameUpdate, _: &mut Self::Context) -> Self::Result {
            self.received.set(self.received.get() + 1);
            let gate = self.gate.take();
            AtomicResponse::new(Box::pin(
                async move {
                    if let Some(gate) = gate {
                        gate.await.unwrap();
                    }
                }
                .into_actor(self),
            ))
        }
    }

    async fn wait_until(mut condition: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while !condition() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    async fn wait_stopped(capturer: &Addr<Capturer>) {
        wait_until(|| !capturer.connected()).await;
    }

    #[actix::test]
    async fn drops_incoming_frames_when_processor_mailbox_is_full() {
        let received = Rc::new(Cell::new(0));
        let (release, gate) = oneshot::channel();
        let processor = RecordingProcessor {
            received: received.clone(),
            gate: Some(gate),
        }
        .start();
        let first = processor.send(FrameUpdate(Frame::dummy(4, 4).unwrap()));
        wait_until(|| received.get() == 1).await;
        let polled = Arc::new(AtomicUsize::new(0));
        let count = polled.clone();
        let frames =
            stream::iter((0..8).map(|_| Ok(Frame::dummy(4, 4).unwrap()))).inspect(move |_| {
                count.fetch_add(1, Ordering::Relaxed);
            });
        let capturer = Capturer::spawn(frames, processor.clone().recipient(), completion());

        wait_stopped(&capturer).await;
        assert_eq!(polled.load(Ordering::Relaxed), 8);
        assert_eq!(received.get(), 1);
        release.send(()).unwrap();
        first.await.unwrap();
        processor
            .send(FrameUpdate(Frame::dummy(4, 4).unwrap()))
            .await
            .unwrap();
        // Initial blocked frame, one queued capture frame, and the final awaited frame.
        assert_eq!(received.get(), 3);
    }

    #[actix::test]
    async fn closed_processor_mailbox_stops_capture() {
        let context = Context::<RecordingProcessor>::new();
        let processor = context.address().recipient();
        drop(context);
        let polled = Arc::new(AtomicUsize::new(0));
        let count = polled.clone();
        let frames = stream::iter([
            Ok(Frame::dummy(4, 4).unwrap()),
            Ok(Frame::dummy(4, 4).unwrap()),
        ])
        .inspect(move |_| {
            count.fetch_add(1, Ordering::Relaxed);
        });
        let capturer = Capturer::spawn(frames, processor, completion());

        wait_stopped(&capturer).await;
        assert_eq!(polled.load(Ordering::Relaxed), 1);
    }

    #[actix::test]
    async fn capture_failure_stops_before_dispatching_more_frames() {
        let received = Rc::new(Cell::new(0));
        let processor = RecordingProcessor {
            received: received.clone(),
            gate: None,
        }
        .start()
        .recipient();
        let frames = stream::iter([
            Err(anyhow::anyhow!("test capture failure")),
            Ok(Frame::dummy(4, 4).unwrap()),
        ]);
        let capturer = Capturer::spawn(frames, processor, completion());

        wait_stopped(&capturer).await;
        assert_eq!(received.get(), 0);
    }

    #[actix::test]
    async fn stop_drops_input_while_processing_is_pending() {
        let received = Rc::new(Cell::new(0));
        let (release, gate) = oneshot::channel();
        let processor = RecordingProcessor {
            received: received.clone(),
            gate: Some(gate),
        }
        .start()
        .recipient();
        let (resource, dropped) = oneshot::channel::<()>();
        let frames = stream::unfold(resource, |resource| async move {
            Some((Ok(Frame::dummy(4, 4).unwrap()), resource))
        });
        let capturer = Capturer::spawn(frames, processor, completion());

        wait_until(|| received.get() == 1).await;
        tokio::time::timeout(Duration::from_secs(1), capturer.send(Stop))
            .await
            .unwrap()
            .unwrap();
        wait_stopped(&capturer).await;
        assert!(dropped.await.is_err());
        let _ = release.send(());
    }
}
