//! Push WLED status with bounded reconnects and periodic HTTP resynchronization.
use super::backend::{LightingService, LightingSnapshot, WledStatus};
use anyhow::{Context as _, Result, bail};
use futures::{SinkExt, StreamExt};
use prysm_contract::ConfigRevision;
use std::time::Duration;
use tokio::{
    sync::watch,
    time::{Instant, interval, interval_at, sleep, timeout},
};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};

const RESYNC_INTERVAL: Duration = Duration::from_secs(30);
const IO_TIMEOUT: Duration = Duration::from_secs(3);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

impl LightingService {
    pub(super) async fn observations(
        &self,
        updates: watch::Sender<Option<(ConfigRevision, LightingSnapshot)>>,
    ) {
        let mut config = self.config.clone();
        loop {
            let current = config.borrow_and_update().clone();
            let Some(url) = current.output.wled_url() else {
                return;
            };
            let publish = |status| {
                updates.send_replace(Some((current.revision, self.status_snapshot(status))));
            };
            let poll = async {
                let mut tick = interval(RESYNC_INTERVAL);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tick.tick().await;
                    let pushed = updates.subscribe();
                    let status = self.wled_status(url).await;
                    // A delayed HTTP response must not replace a newer pushed state.
                    if !pushed.has_changed().unwrap_or(true) {
                        publish(status);
                    }
                }
            };
            tokio::select! {
                result = config.changed() => {
                    if result.is_err() { return; }
                }
                () = poll => return,
                () = websocket_updates(url, |status| publish(Ok(status))) => return,
            }
        }
    }
}

async fn websocket_updates(url: &reqwest::Url, mut publish: impl FnMut(WledStatus)) {
    let mut retry_delay = Duration::from_secs(1);
    loop {
        let result = websocket_session(url, |status| {
            retry_delay = Duration::from_secs(1);
            publish(status);
        })
        .await;
        if let Err(error) = result {
            tracing::debug!(error = %format!("{error:#}"), ?retry_delay, "WLED WebSocket disconnected; retrying");
        }
        sleep(retry_delay).await;
        retry_delay = (retry_delay * 2).min(MAX_RETRY_DELAY);
    }
}

async fn websocket_session(url: &reqwest::Url, mut publish: impl FnMut(WledStatus)) -> Result<()> {
    let mut url = url.join("ws")?;
    let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    url.set_scheme(scheme)
        .map_err(|()| anyhow::anyhow!("invalid WLED WebSocket URL"))?;
    let limits = WebSocketConfig::default()
        .max_message_size(Some(64 * 1024))
        .max_frame_size(Some(64 * 1024));
    let (mut socket, _) = timeout(
        IO_TIMEOUT,
        connect_async_with_config(url.as_str(), Some(limits), false),
    )
    .await
    .context("WLED WebSocket connection timed out")?
    .context("cannot connect to WLED WebSocket")?;
    let mut heartbeat = interval_at(Instant::now() + HEARTBEAT_INTERVAL, HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let idle = sleep(IDLE_TIMEOUT);
    tokio::pin!(idle);
    loop {
        tokio::select! {
            () = &mut idle => bail!("WLED WebSocket heartbeat timed out"),
            _ = heartbeat.tick() => {
                // WLED's application heartbeat works on firmware that ignores protocol pings.
                timeout(IO_TIMEOUT, socket.send(Message::Text("p".into())))
                    .await.context("WLED WebSocket heartbeat send timed out")??;
            }
            message = socket.next() => {
                let message = message.context("WLED WebSocket closed")??;
                idle.as_mut().reset(Instant::now() + IDLE_TIMEOUT);
                match message {
                    Message::Text(text) if text == "pong" => {}
                    Message::Text(text) => {
                        let value: serde_json::Value = serde_json::from_str(&text)
                            .context("invalid WLED WebSocket JSON")?;
                        if value.get("state").is_some() || value.get("info").is_some() {
                            publish(serde_json::from_value(value).context("invalid WLED WebSocket status")?);
                        } else if let Some(error) = value.get("error") {
                            bail!("WLED WebSocket error: {error}");
                        }
                    }
                    Message::Close(_) => bail!("WLED WebSocket closed"),
                    Message::Ping(_) => {
                        timeout(IO_TIMEOUT, socket.flush())
                            .await.context("WLED WebSocket pong send timed out")??;
                    }
                    Message::Pong(_) => {}
                    _ => bail!("unexpected WLED WebSocket message"),
                }
            }
        }
    }
}

#[cfg(all(test, feature = "host-input"))]
mod tests;
