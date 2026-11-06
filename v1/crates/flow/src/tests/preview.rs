use super::*;
use crate::preview::Publisher;
use prysm_contract::preview::PreviewFrame;
use prysm_core::{EdgeColors, FramePool, ProcessedSnapshot, Viewport};

fn snapshot(frame: Frame, sequence: u64) -> PreviewFrame {
    PreviewFrame {
        stamp: super::frame(1, 1, sequence, 0).stamp,
        revision: SettingsRevision(7),
        snapshot: ProcessedSnapshot {
            frame,
            viewport: None,
            colors: EdgeColors::default(),
        },
    }
}

#[test]
fn waking_a_viewer_can_synchronously_drop_its_lease_on_publication_and_closure() {
    use futures::{
        Stream,
        task::{ArcWake, waker_ref},
    };
    use prysm_contract::preview::PreviewLease;
    use std::{pin::Pin, task::Context};

    struct CancelOnWake(Mutex<Option<PreviewLease>>);
    impl ArcWake for CancelOnWake {
        fn wake_by_ref(this: &Arc<Self>) {
            let lease = this.0.lock().unwrap().take();
            drop(lease);
        }
    }
    for close in [false, true] {
        let (publisher, handle) = Publisher::new();
        let lease = futures::executor::block_on(handle.subscribe()).unwrap();
        let cancel = Arc::new(CancelOnWake(Mutex::new(Some(lease))));
        let waker = waker_ref(&cancel);
        let mut context = Context::from_waker(&waker);
        assert!(
            Pin::new(cancel.0.lock().unwrap().as_mut().unwrap())
                .poll_next(&mut context)
                .is_pending()
        );
        let (completed, done) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            if !close {
                publisher.publish(snapshot(Frame::dummy(8, 4).unwrap(), 1));
            }
            drop(publisher);
            completed.send(()).unwrap();
        });
        done.recv_timeout(Duration::from_secs(5))
            .expect("preview wake deadlocked while dropping a lease");
        worker.join().unwrap();
        assert!(cancel.0.lock().unwrap().is_none());
        assert!(futures::executor::block_on(handle.subscribe()).is_err());
    }
}

#[tokio::test]
async fn only_live_leases_retain_pixels_and_final_drop_clears_unpolled_publication() {
    let (publisher, handle) = Publisher::new();
    let mut pool = FramePool::default();
    let source = pool.acquire(8, 4, PixelFormat::RGB24).unwrap().finish();
    let pixels = source.as_slice().as_ptr();
    assert!(publisher.retain(&source).is_none());
    publisher.publish(snapshot(source, 1));
    let source = pool.acquire(8, 4, PixelFormat::RGB24).unwrap().finish();
    assert_eq!(source.as_slice().as_ptr(), pixels);

    let lease = handle.subscribe().await.unwrap();
    let mut other = handle.subscribe().await.unwrap();
    publisher.publish(snapshot(source, 2));
    drop(lease);
    let latest = within(other.next()).await.unwrap().unwrap();
    assert_eq!(latest.snapshot.frame.as_slice().as_ptr(), pixels);
    drop(latest);
    drop(other);
    let source = pool.acquire(8, 4, PixelFormat::RGB24).unwrap().finish();
    assert_eq!(source.as_slice().as_ptr(), pixels);

    let lease = handle.subscribe().await.unwrap();
    let retained = publisher.retain(&source).unwrap();
    drop(source);
    drop(lease);
    // A processing pass that began with demand cannot repopulate an unused tap.
    publisher.publish(snapshot(retained, 3));
    let mut fresh = handle.subscribe().await.unwrap();
    assert!(fresh.next().now_or_never().is_none());
    let source = pool.acquire(8, 4, PixelFormat::RGB24).unwrap().finish();
    assert_eq!(source.as_slice().as_ptr(), pixels);
    publisher.publish(snapshot(source, 4));
    drop(publisher);
    assert!(within(fresh.next()).await.is_none());
    assert!(handle.subscribe().await.is_err());
    let source = pool.acquire(8, 4, PixelFormat::RGB24).unwrap().finish();
    assert_eq!(source.as_slice().as_ptr(), pixels);
}

#[tokio::test]
async fn last_lease_drop_racing_publish_never_leaves_a_retained_snapshot() {
    let (publisher, handle) = Publisher::new();
    for sequence in 1..=32 {
        let lease = handle.subscribe().await.unwrap();
        let frame = snapshot(Frame::dummy(8, 4).unwrap(), sequence);
        let start = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                start.wait();
                publisher.publish(frame);
            });
            start.wait();
            drop(lease);
        });
        let mut fresh = handle.subscribe().await.unwrap();
        assert!(fresh.next().now_or_never().is_none());
    }
}

#[tokio::test]
async fn slow_preview_coalesces_coherent_processing_passes_without_blocking_colors() {
    let (sink, mut colors, _) = sink(None);
    let mut flow = Running::start(sink).await;
    let mut preview = flow.preview.subscribe().await.unwrap();
    assert!(preview.next().now_or_never().is_none());
    assert_eq!(
        *flow.reads.borrow(),
        1,
        "subscribing does not start capture"
    );
    flow.configure(7, config()).await;
    for sequence in 1..=20 {
        flow.send(frame(1, 1, sequence, 255)).await;
        let output = within(colors.recv()).await.unwrap();
        assert_eq!(output.stamp.sequence, sequence);
    }
    // Ordered observation proves the last pass finished publishing its preview.
    flow.state().await;
    let latest = within(preview.next()).await.unwrap().unwrap();
    assert_eq!(latest.stamp.sequence, 20);
    assert_eq!(latest.revision, SettingsRevision(7));
    assert!(
        latest
            .snapshot
            .frame
            .as_slice()
            .iter()
            .all(|&byte| byte == 255)
    );
    assert_eq!(latest.snapshot.viewport, Some(Viewport::full_frame(8, 4)));
    assert_eq!(latest.snapshot.colors.top.sample_at(0.5).r, 1.0);

    let mut second = flow.preview.subscribe().await.unwrap();
    assert_eq!(
        within(second.next()).await.unwrap().unwrap().stamp,
        latest.stamp
    );
    flow.configure(
        8,
        Config {
            brightness_percent: BrightnessPercent(0),
            ..config()
        },
    )
    .await;
    flow.send(frame(1, 1, 21, 255)).await;
    let output = within(colors.recv()).await.unwrap();
    let latest = within(preview.next()).await.unwrap().unwrap();
    assert_eq!(latest.stamp, output.stamp);
    assert_eq!(latest.revision, output.revision);
    assert_eq!(
        latest.snapshot.colors.top.sample_at(0.5),
        output.colors.top.sample_at(0.5)
    );
    assert_eq!(latest.snapshot.colors.top.sample_at(0.5).r, 0.0);
    assert_eq!(latest.snapshot.frame.as_slice()[0], 255);

    within(flow.control.barrier(session(1))).await.unwrap();
    flow.send(frame(1, 1, 22, 0)).await;
    flow.state().await;
    assert!(preview.next().now_or_never().is_none());
    flow.stop().await;
    assert!(within(preview.next()).await.is_none());
    assert!(
        within(second.next()).await.is_none(),
        "closure clears unseen snapshots"
    );
}

#[tokio::test]
async fn dropping_a_never_run_owner_or_unpolled_run_closes_preview() {
    for call_run in [false, true] {
        let (_sender, frames) = mpsc::channel(1);
        let (sink, _, _) = sink(None);
        let flow = ProcessingFlow::connect(
            Source {
                frames,
                reads: watch::channel(0).0,
                cancelled: watch::channel(0).0,
            },
            PrysmProcessor::default(),
            sink,
            ProcessingExecution::DedicatedThread,
        );
        let mut preview = flow.preview.subscribe().await.unwrap();
        assert!(preview.next().now_or_never().is_none());
        if call_run {
            drop(flow.owner.run(INSTANCE, std::future::pending()));
        } else {
            drop(flow.owner);
        }
        assert!(within(preview.next()).await.is_none());
        assert!(flow.preview.subscribe().await.is_err());
    }
}
