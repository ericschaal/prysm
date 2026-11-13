//! Listener and connection ownership. Every connection task is joined (or,
//! past the drain deadline, aborted) before `serve` returns.
use crate::WebPortal;
use crate::routes::{self, BODY_DEADLINE, Context};
use crate::timed_io::TimedIo;
use hyper::{server::conn::http1, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use prysm_ports::{Error, Result};
use prysm_runtime::{EngineHandle, Shutdown};
use std::convert::Infallible;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;
use tokio::time::timeout;

const CONNECTIONS: usize = 64;
/// How long open requests may take to finish once shutdown starts.
const DRAIN_DEADLINE: Duration = Duration::from_secs(5);

pub(crate) async fn serve(
    portal: WebPortal,
    engine: EngineHandle,
    shutdown: Shutdown,
) -> Result<()> {
    let listener = portal
        .listener
        .set_nonblocking(true)
        .and_then(|()| TcpListener::from_std(portal.listener))
        .map_err(|error| Error::adapter("cannot use the portal listener", error))?;
    let context = Arc::new(Context::new(
        engine,
        portal.assets,
        portal.access,
        portal.metrics,
        portal.capture,
        shutdown.clone(),
    ));
    let mut connections = JoinSet::new();
    let result = loop {
        tokio::select! {
            biased;
            () = shutdown.requested() => break Ok(()),
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
            accepted = listener.accept(), if connections.len() < CONNECTIONS => match accepted {
                Ok((socket, _)) => {
                    connections.spawn(connection(socket, context.clone()));
                }
                Err(error) if transient(&error) => {}
                Err(error) => break Err(Error::adapter("portal listener failed", error)),
            },
        }
    };
    // Refuse new connections at once; open ones finish their current request.
    drop(listener);
    let drained = timeout(DRAIN_DEADLINE, async {
        while connections.join_next().await.is_some() {}
    })
    .await;
    if drained.is_err() {
        connections.shutdown().await;
    }
    result
}

/// Failures of one pending connection, not of the listener.
fn transient(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::Interrupted
            | io::ErrorKind::WouldBlock
    )
}

async fn connection(socket: TcpStream, context: Arc<Context>) {
    let shutdown = context.shutdown.clone();
    let service = service_fn(move |request| {
        let context = context.clone();
        async move { Ok::<_, Infallible>(routes::handle(context, request).await) }
    });
    let connection = http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(BODY_DEADLINE)
        .serve_connection(TokioIo::new(TimedIo::new(socket)), service)
        .with_upgrades();
    tokio::pin!(connection);
    tokio::select! {
        _ = connection.as_mut() => {}
        () = shutdown.requested() => {
            connection.as_mut().graceful_shutdown();
            let _ = connection.await;
        }
    }
}
