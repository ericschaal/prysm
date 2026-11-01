use super::{RenderFrame, Stop, pipeline::Completion};
use actix::{Actor, ActorContext, Context, Handler};
use anyhow::{Context as _, Result};
use prysm_contract::{LedCounts, WledAddress};
use prysm_renderer_wled::WledRenderer;

/// WLED output. Other outputs handle the same `RenderFrame` message.
pub struct Wled {
    backend: WledRenderer,
    leds: LedCounts,
    completion: Completion,
}

impl Wled {
    pub fn new(address: &WledAddress, leds: LedCounts, completion: Completion) -> Result<Self> {
        let backend = WledRenderer::new(address.as_str()).context("cannot create WLED renderer")?;
        Ok(Self {
            backend,
            leds,
            completion,
        })
    }
}

impl Actor for Wled {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.set_mailbox_capacity(1);
    }
}

impl Handler<Stop> for Wled {
    type Result = ();

    fn handle(&mut self, _: Stop, ctx: &mut Self::Context) {
        ctx.stop();
    }
}

impl Handler<RenderFrame> for Wled {
    type Result = ();

    fn handle(&mut self, msg: RenderFrame, ctx: &mut Context<Self>) {
        if let Err(error) = self
            .backend
            .render_edges(&msg.colors, self.leds.counts())
            .context("cannot send WLED frame")
        {
            tracing::error!(error = %format!("{error:#}"), "rendering failed");
            let _ = self.completion.try_send(Err(error));
            ctx.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix::Recipient;
    use prysm_capture::Frame;
    use prysm_core::{ColorStrip, EdgeColors, LinearColor};
    use std::time::Duration;

    #[actix::test]
    async fn recipient_renders_edges_in_physical_led_order() {
        let receiver = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let config: prysm_contract::Output = serde_json::from_value(serde_json::json!({
            "type": "wled",
            "address": receiver.local_addr().unwrap().to_string(),
            "leds": { "top": 2, "right": 1, "bottom": 1, "left": 1 }
        }))
        .unwrap();
        let prysm_contract::Output::Wled { address, leds } = config else {
            panic!("expected WLED fixture")
        };
        let output: Recipient<RenderFrame> =
            Wled::new(&address, leds, tokio::sync::mpsc::channel(3).0)
                .unwrap()
                .start()
                .recipient();
        let edges = EdgeColors::new(
            ColorStrip::fill(LinearColor::new(1.0, 0.0, 0.0), std::num::NonZeroUsize::MIN),
            ColorStrip::fill(LinearColor::new(0.0, 1.0, 0.0), std::num::NonZeroUsize::MIN),
            ColorStrip::fill(LinearColor::new(0.0, 0.0, 1.0), std::num::NonZeroUsize::MIN),
            ColorStrip::fill(LinearColor::new(1.0, 1.0, 1.0), std::num::NonZeroUsize::MIN),
        );

        output
            .send(RenderFrame {
                frame: Frame::dummy(4, 4).unwrap(),
                viewport: None,
                colors: edges,
            })
            .await
            .unwrap();

        let mut packet = [0; 1500];
        let size = tokio::time::timeout(Duration::from_secs(1), receiver.recv(&mut packet))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(size, 10 + 5 * 3);
        // DDP's ten-byte header precedes left, top, right, bottom RGB pixels.
        assert_eq!(
            &packet[10..size],
            &[255, 255, 255, 255, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255]
        );
    }
}
