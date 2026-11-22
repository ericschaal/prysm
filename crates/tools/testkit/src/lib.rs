//! What the workspace's tests share: bounded waits, child processes, schema
//! checks and the host launcher ([`host`]). A dev-dependency only.
//!
//! Every wait here is bounded and fails the test with what it waited for.
//! A wait that can block on the thing itself (a channel, a watch, a socket
//! read with a timeout) should do that instead of [`eventually`], which looks
//! again every few milliseconds.
pub mod host;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::fmt;
use std::future::Future;
use std::process::{Child, ExitStatus};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};
use utoipa::ToSchema;

/// How long [`eventually`] and [`eventually_async`] wait between looks.
const POLL: Duration = Duration::from_millis(5);

/// The first `Some` that `probe` returns, looking every few milliseconds.
///
/// # Panics
/// `probe` returned `None` for `limit`; the message names `what`.
pub fn eventually<T>(what: &str, limit: Duration, mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(found) = probe() {
            return found;
        }
        assert!(Instant::now() < deadline, "{what}: not within {limit:?}");
        thread::sleep(POLL);
    }
}

/// [`eventually`] for async tests: it sleeps on the runtime between looks.
///
/// # Panics
/// `probe` returned `None` for `limit`; the message names `what`.
pub async fn eventually_async<T>(
    what: &str,
    limit: Duration,
    mut probe: impl FnMut() -> Option<T>,
) -> T {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(found) = probe() {
            return found;
        }
        assert!(Instant::now() < deadline, "{what}: not within {limit:?}");
        tokio::time::sleep(POLL).await;
    }
}

/// Run `work` on its own thread and return its result, failing the test
/// instead of hanging; a panic in `work` is the test's panic.
///
/// # Panics
/// `work` panicked, or did not finish within `limit`.
pub fn within<T: Send + 'static>(limit: Duration, work: impl FnOnce() -> T + Send + 'static) -> T {
    let (done, result) = mpsc::channel();
    let worker = thread::spawn(move || {
        let _ = done.send(work());
    });
    match result.recv_timeout(limit) {
        Ok(value) => value,
        Err(RecvTimeoutError::Disconnected) => match worker.join() {
            Err(panic) => std::panic::resume_unwind(panic),
            Ok(()) => unreachable!("the worker sends before it ends"),
        },
        Err(RecvTimeoutError::Timeout) => panic!("the test did not finish within {limit:?}"),
    }
}

/// `test`'s output, failing the test instead of hanging.
///
/// # Panics
/// `test` did not finish within `limit`.
pub async fn within_async<F: Future>(limit: Duration, test: F) -> F::Output {
    tokio::time::timeout(limit, test)
        .await
        .unwrap_or_else(|_| panic!("the test did not finish within {limit:?}"))
}

/// Wait up to `limit` for `child` to exit. A child still running then is
/// killed and reaped, and `None` is returned, so the caller's panic can say
/// what it waited for.
///
/// # Panics
/// The child's status cannot be read.
pub fn wait_child(child: &mut Child, limit: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().expect("the child's status") {
            return Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(POLL);
    }
}

/// The schema of `T` (settings with an OpenAPI schema), after checking that
/// it lists exactly the fields `full` serializes, requires exactly those
/// serde cannot default, and rejects unknown fields as serde does.
///
/// # Panics
/// The schema and serde disagree.
pub fn assert_schema_matches_serde<T: ToSchema + Serialize + DeserializeOwned>(full: &T) -> Value {
    let schema = serde_json::to_value(T::schema()).unwrap();
    let properties = schema["properties"].as_object().unwrap();
    let required: Vec<String> =
        serde_json::from_value(schema["required"].clone()).unwrap_or_default();
    let Value::Object(fields) = serde_json::to_value(full).unwrap() else {
        panic!("settings serialize as an object");
    };
    let keys = |map: &Map<String, Value>| map.keys().cloned().collect::<BTreeSet<_>>();
    assert_eq!(keys(&fields), keys(properties));
    let minimal: Map<String, Value> = fields
        .iter()
        .filter(|(key, _)| required.contains(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    serde_json::from_value::<T>(minimal.clone().into()).unwrap();
    for key in &required {
        let mut missing = minimal.clone();
        missing.remove(key);
        assert!(
            serde_json::from_value::<T>(missing.into()).is_err(),
            "{key}"
        );
    }
    assert_eq!(schema["additionalProperties"], json!(false));
    let mut unknown = fields;
    unknown.insert("unknown".into(), json!(1));
    assert!(serde_json::from_value::<T>(unknown.into()).is_err());
    schema
}

/// The schema mirror `M` reads what `P` writes, and `P` reads it back.
///
/// # Panics
/// The two disagree on `value`.
pub fn assert_mirror_round_trips<P, M>(value: &P)
where
    P: Serialize + DeserializeOwned + PartialEq + fmt::Debug,
    M: Serialize + DeserializeOwned,
{
    let json = serde_json::to_value(value).unwrap();
    let mirror: M = serde_json::from_value(json.clone()).unwrap();
    let back = serde_json::to_value(&mirror).unwrap();
    assert_eq!(back, json);
    assert_eq!(&serde_json::from_value::<P>(back).unwrap(), value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eventually_returns_the_first_value_found() {
        let mut looks = 0;
        let found = eventually("the third look", Duration::from_secs(5), || {
            looks += 1;
            (looks == 3).then_some(looks)
        });
        assert_eq!(found, 3);
    }

    #[test]
    #[should_panic(expected = "nothing: not within")]
    fn eventually_names_what_it_waited_for() {
        eventually::<()>("nothing", Duration::ZERO, || None);
    }

    #[test]
    #[should_panic(expected = "the work's own panic")]
    fn within_passes_a_panic_on() {
        within(Duration::from_secs(5), || panic!("the work's own panic"));
    }

    #[test]
    fn wait_child_kills_a_child_that_does_not_exit() {
        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .unwrap();
        assert!(wait_child(&mut child, Duration::ZERO).is_none());
        assert!(child.try_wait().unwrap().is_some(), "killed and reaped");
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let status = wait_child(&mut child, Duration::from_secs(5)).unwrap();
        assert!(status.success());
    }
}
