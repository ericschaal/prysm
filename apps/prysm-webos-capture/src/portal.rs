//! Embedded application adapter: HTTP is process-owned; LG resources are session-owned.
use crate::{
    capture::Capture,
    config::{CaptureOptions, Source},
    control::Control,
    events::{Event, Events},
    lock::capture_lock,
    native, rgb, shutdown,
};
use anyhow::{Context, Result, anyhow, ensure};
use futures::StreamExt;
use nix::{ifaddrs::getifaddrs, net::if_::InterfaceFlags};
use prysm_core::{Frame, FramePool};
use prysm_runtime::{InputStream, NativeCapture, WebConfig};
use prysm_webos_protocol::{WORKER_CONFIG_ENV, worker::EmbeddedWorkerConfig};
use std::{
    net::{Ipv4Addr, SocketAddr},
    path::Path,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

const PORT: u16 = 8787;

fn lan_addresses() -> Result<Vec<Ipv4Addr>> {
    let mut addresses: Vec<_> = getifaddrs()?
        .filter(|iface| {
            iface.flags.contains(InterfaceFlags::IFF_UP)
                && !iface.flags.contains(InterfaceFlags::IFF_LOOPBACK)
        })
        .filter_map(|iface| {
            iface
                .address?
                .as_sockaddr_in()
                .map(nix::sys::socket::SockaddrIn::ip)
        })
        .filter(|ip| {
            !ip.is_loopback() && !ip.is_unspecified() && !ip.is_multicast() && !ip.is_broadcast()
        })
        .collect();
    // A UDP connect selects a source through the routing table without sending data.
    let preferred = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .and_then(|socket| {
            socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9))?;
            socket.local_addr()
        })
        .ok()
        .and_then(|address| match address {
            SocketAddr::V4(address) => Some(*address.ip()),
            SocketAddr::V6(_) => None,
        });
    addresses.sort_by_key(|ip| (Some(*ip) != preferred, ip.is_link_local(), *ip));
    addresses.dedup();
    ensure!(
        !addresses.is_empty(),
        "no IPv4 LAN address for the TV portal"
    );
    Ok(addresses)
}

#[derive(Clone, Copy, Default)]
struct VideoState {
    available: bool,
    generation: u64,
}

#[derive(Clone)]
struct TvInput {
    source: Source,
    video: watch::Receiver<VideoState>,
    events: Arc<Mutex<Events>>,
    /// Held from before opening the LG tap until after closing it and releasing the process lock.
    tap: Arc<tokio::sync::Mutex<()>>,
}

impl TvInput {
    fn events(&self) -> Result<MutexGuard<'_, Events>> {
        self.events
            .lock()
            .map_err(|_| anyhow!("capture event channel poisoned"))
    }

    fn capture(
        &self,
        options: CaptureOptions,
        stop: &CancellationToken,
        frames: &mpsc::Sender<Result<Frame>>,
        availability: &watch::Sender<bool>,
    ) -> Result<()> {
        let period = Duration::from_secs(1) / u32::from(options.fps);
        // Drop the driver before its exclusivity guards even when event I/O fails.
        let mut held = None;
        let mut capture = Capture::with_options(options, self.source);
        let mut pool = FramePool::default();
        let mut generation = self.video.borrow().generation;
        let mut next = Instant::now();
        let result = (|| -> Result<()> {
            while !stop.is_cancelled() && !frames.is_closed() {
                let state = *self.video.borrow();
                if !state.available || generation != state.generation {
                    if held.is_some() {
                        capture.release_video(&mut *self.events()?)?;
                        held = None;
                        self.idle(availability)?;
                    }
                    generation = state.generation;
                }
                if state.available {
                    let frame = capture.capture(
                        &mut |opts| {
                            let admission = self.tap.clone().blocking_lock_owned();
                            ensure!(self.video.borrow().available, "capture paused");
                            let lock = capture_lock(Path::new("/tmp/prysm-capture.lock"))?;
                            let video = native::Video::open(opts, self.source)?;
                            held = Some((lock, admission));
                            Ok(video)
                        },
                        &mut *self.events()?,
                    )?;
                    if let Some(frame) = frame {
                        let frame = rgb(&mut pool, frame.header, frame.pixels)?;
                        if frames.blocking_send(Ok(frame)).is_err() {
                            break;
                        }
                        if availability.send_if_modified(|v| !std::mem::replace(v, true)) {
                            self.events()?.send(&Event::Capturing)?;
                        }
                    } else if !capture.has_video() {
                        held = None;
                        self.idle(availability)?;
                    }
                }
                next += period;
                let now = Instant::now();
                if now.saturating_duration_since(next) >= period {
                    next = now;
                }
                std::thread::sleep(next.saturating_duration_since(now));
            }
            Ok(())
        })();
        if let Err(error) = &result {
            if error.is::<native::FatalVideo>() {
                self.events()?.send(&Event::Failed {
                    message: format!("{error:#}"),
                })?;
            }
            let _ = frames.blocking_send(Err(anyhow!("{error:#}")));
        }
        let released = capture.release_video(&mut *self.events()?);
        drop(capture);
        drop(held);
        self.idle(availability)?;
        result.and(released)
    }

    fn idle(&self, availability: &watch::Sender<bool>) -> Result<()> {
        if availability.send_if_modified(|v| std::mem::replace(v, false)) {
            self.events()?.send(&Event::CaptureIdle)?;
        }
        Ok(())
    }
}

impl NativeCapture for TvInput {
    fn open(
        &self,
        resolution: prysm_contract::CaptureResolution,
        rate: std::num::NonZeroU32,
        stop: CancellationToken,
    ) -> Result<InputStream> {
        let [width, height] = resolution.dimensions();
        let options = CaptureOptions {
            width,
            height,
            fps: u8::try_from(rate.get())?,
        };

        let (frames, rx) = mpsc::channel(1);
        let (available, state) = watch::channel(false);
        let native = self.clone();
        let cleanup = tokio::task::spawn_blocking(move || {
            native.capture(options, &stop, &frames, &available)
        });
        Ok(InputStream::native(
            futures::stream::unfold(rx, |mut rx| async move { Some((rx.recv().await?, rx)) })
                .boxed(),
            cleanup,
            state,
        ))
    }
}

// Only this task reads the supervisor pipe. A generation preserves a reset that
// occurs between two polls by the native capture thread.
async fn supervise(
    mut control: Control,
    video: watch::Sender<VideoState>,
    stop: Arc<AtomicBool>,
    addresses: &[Ipv4Addr],
    input: &TvInput,
) -> Result<()> {
    let mut recheck = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let (update, pause) = control.poll_with_pause()?;
        video.send_modify(|state| {
            state.available = update.availability.allows_capture();
            if update.reset {
                state.generation = state.generation.wrapping_add(1);
            }
        });
        if pause {
            // The new admission state is visible before this barrier. A native open racing
            // the pause must re-check that state while it owns the same mutex.
            drop(input.tap.lock().await);
            input.events()?.send(&Event::CapturePaused)?;
        }
        if recheck.elapsed() >= Duration::from_secs(5) {
            ensure!(
                lan_addresses()? == addresses,
                "TV network addresses changed; reopening portal"
            );
            recheck = Instant::now();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

pub(super) fn serve(path: &Path) -> Result<()> {
    let events = Arc::new(Mutex::new(Events::from_environment()?));
    let result = (|| {
        ensure!(
            cfg!(target_os = "linux") && nix::unistd::Uid::effective().is_root(),
            "TV capture requires webOS root"
        );
        let (stop, _signal_thread) = shutdown::listen()?;
        let config: EmbeddedWorkerConfig = serde_json::from_str(
            &std::env::var(WORKER_CONFIG_ENV).context("missing worker configuration")?,
        )?;
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(std::io::stderr)
            .init();
        let _metrics = crate::metrics::exporter(config.metrics);
        let addresses = lan_addresses()?;
        let listen = SocketAddr::from((Ipv4Addr::UNSPECIFIED, PORT));
        let advertised = SocketAddr::from((addresses[0], PORT));
        let mut allowed_hosts: Vec<_> = addresses.iter().map(|ip| format!("{ip}:{PORT}")).collect();
        allowed_hosts.extend([format!("127.0.0.1:{PORT}"), format!("localhost:{PORT}")]);
        let web = WebConfig {
            listen,
            allowed_hosts,
        };
        let (state, video) = watch::channel(VideoState::default());
        let input = Arc::new(TvInput {
            source: config.source,
            video,
            events: events.clone(),
            tap: Arc::default(),
        });
        let control = Control::from_stdin()?;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async {
                let cancel = CancellationToken::new();
                let portal = prysm_runtime::run_native(
                    path.to_owned(),
                    web,
                    input.clone(),
                    cancel.clone(),
                    |bound| {
                        ensure!(bound.port() == advertised.port(), "unexpected portal port");
                        input.events()?.send(&Event::PortalReady {
                            address: advertised,
                        })
                    },
                );
                tokio::pin!(portal);
                tokio::select! {
                    result=&mut portal => result,
                    result=supervise(control,state,stop,&addresses,&input) => {
                        cancel.cancel();
                        let exit=portal.await;
                        result.and(exit)
                    }
                }
            })
    })();
    if let Err(error) = &result
        && let Ok(mut sink) = events.lock()
    {
        sink.send(&Event::Failed {
            message: format!("{error:#}"),
        })?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write as _,
        os::{fd::OwnedFd, unix::net::UnixStream},
    };
    use tokio::io::{AsyncBufReadExt, BufReader};

    #[tokio::test]
    async fn pause_ack_waits_for_opening_tap_and_also_works_with_no_session() {
        let (read, mut commands) = std::io::pipe().unwrap();
        let control = Control::new(OwnedFd::from(read).into()).unwrap();
        let (writer, reader) = UnixStream::pair().unwrap();
        let events = Events::from_file(OwnedFd::from(writer).into());
        reader.set_nonblocking(true).unwrap();
        let mut lines = BufReader::new(tokio::net::UnixStream::from_std(reader).unwrap()).lines();
        let (state, mut video) = watch::channel(VideoState {
            available: true,
            generation: 0,
        });
        let input = TvInput {
            source: Source::Video,
            events: Arc::new(Mutex::new(events)),
            tap: Arc::default(),
            video: video.clone(),
        };
        let tap = input.tap.clone().lock_owned().await;
        commands.write_all(b"p").unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let owner =
            tokio::spawn(async move { supervise(control, state, stopped, &[], &input).await });
        video.wait_for(|state| !state.available).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), lines.next_line())
                .await
                .is_err()
        );
        drop(tap);
        let mut ack = async || {
            let line = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Event>(&line).unwrap(),
                Event::CapturePaused
            );
        };
        ack().await;
        // The portal still acknowledges a new pause when there is no native session.
        commands.write_all(b"p").unwrap();
        ack().await;
        stop.store(true, Ordering::Relaxed);
        owner.await.unwrap().unwrap();
    }
}
