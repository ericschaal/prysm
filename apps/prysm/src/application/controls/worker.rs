//! Background HTTP requests for the native toolbar.
use super::TargetRequest;
use prysm_api::client::{get_state, set_target};
use prysm_contract::{ApiError, StateResponse};
use std::{net::SocketAddr, time::Duration};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

fn endpoint(mut address: SocketAddr) -> String {
    if address.ip().is_unspecified() {
        address.set_ip(match address {
            SocketAddr::V4(_) => std::net::Ipv4Addr::LOCALHOST.into(),
            SocketAddr::V6(_) => std::net::Ipv6Addr::LOCALHOST.into(),
        });
    }
    format!("http://{address}")
}

pub(super) async fn run(
    http: reqwest::Client,
    address: SocketAddr,
    state: watch::Sender<Option<Result<StateResponse, ApiError>>>,
    mut targets: mpsc::Receiver<TargetRequest>,
    shutdown: CancellationToken,
) {
    let url = endpoint(address);
    let mut poll = tokio::time::interval(Duration::from_millis(250));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            _ = poll.tick() => {
                state.send_replace(Some(get_state(&http, &url).await));
            }
            request = targets.recv() => {
                let Some((target, reply)) = request else { break; };
                let result = set_target(&http, &url, target).await;
                // Refresh before unlocking the button so a cached state cannot trigger a duplicate start.
                if result.is_ok() {
                    state.send_replace(Some(get_state(&http, &url).await));
                }
                let _ = reply.send(result);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_listeners_connect_over_loopback() {
        for (listen, url) in [
            ("0.0.0.0:8787", "http://127.0.0.1:8787"),
            ("[::]:8787", "http://[::1]:8787"),
            ("192.0.2.1:8787", "http://192.0.2.1:8787"),
        ] {
            assert_eq!(endpoint(listen.parse().unwrap()), url);
        }
    }
}
