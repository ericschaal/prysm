//! Publish processed frames to the main-thread egui renderer.
use super::{RenderFrame, Stop};
use actix::{Actor, ActorContext, Context, Handler};
use prysm_core::EdgeColors;
use prysm_renderer_desktop::DisplayFrame;
use tokio::sync::watch;

#[derive(Clone)]
pub struct Egui {
    pub colors: watch::Sender<EdgeColors>,
    pub frames: watch::Sender<DisplayFrame>,
}

impl Actor for Egui {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.set_mailbox_capacity(1);
    }
}

impl Handler<RenderFrame> for Egui {
    type Result = ();

    fn handle(&mut self, msg: RenderFrame, _: &mut Self::Context) {
        self.frames.send_replace(DisplayFrame {
            frame: msg.frame,
            viewport: msg.viewport,
        });
        self.colors.send_replace(msg.colors);
    }
}

impl Handler<Stop> for Egui {
    type Result = ();

    fn handle(&mut self, _: Stop, ctx: &mut Self::Context) {
        ctx.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_capture::Frame;
    use prysm_core::{LinearColor, SampleDensity, Viewport};

    #[actix::test]
    async fn publishes_source_crop_and_colors_to_the_gui() {
        let (colors, colors_rx) = watch::channel(EdgeColors::default());
        let (frames, frames_rx) = watch::channel(DisplayFrame {
            frame: Frame::dummy(4, 4),
            viewport: None,
        });
        let renderer = Egui { colors, frames }.start();
        let frame = Frame::fill(255, 4, 4, prysm_capture::PixelFormat::RGB24);
        let viewport = Some(Viewport::full_frame(4, 4));
        let colors = EdgeColors::fill(LinearColor::new(1.0, 0.0, 0.0), 4, 4, SampleDensity(128));
        renderer
            .send(RenderFrame {
                frame: frame.clone(),
                viewport,
                colors: colors.clone(),
            })
            .await
            .unwrap();
        assert!(std::sync::Arc::ptr_eq(
            &frames_rx.borrow().frame.data,
            &frame.data
        ));
        assert_eq!(frames_rx.borrow().viewport, viewport);
        assert_eq!(*colors_rx.borrow(), colors);
        renderer.send(Stop).await.unwrap();
    }
}
