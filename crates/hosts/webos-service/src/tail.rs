//! The supervisor's own log lines for the state's tail. The TV starts the
//! supervisor with stderr on `/dev/null`, so each line goes to stderr and,
//! while a loop drains them, to the tail with the worker's.
use crate::launcher::read_log;
use crate::machine::LOG_LINES;
use prysm_webos_luna_sys::Waker;
use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The log writer of [`prysm_logging::init`] (through [`Tail::writer`]) and
/// the queue between it and the loop. Lines are only queued once a loop is
/// [attached](Tail::attach); before that, and in `setup`, they go to stderr
/// only.
#[derive(Debug, Clone, Default)]
pub(crate) struct Tail(Arc<Mutex<Queue>>);

#[derive(Debug, Default)]
struct Queue {
    /// Woken after each queued event; `None` until a loop drains the queue.
    waker: Option<Waker>,
    /// At most [`LOG_LINES`]; a full queue drops its oldest line, which the
    /// tail would drop anyway behind as many newer lines.
    lines: VecDeque<String>,
}

impl Tail {
    /// Queue lines from now on and wake `waker`'s loop for each event.
    pub(crate) fn attach(&self, waker: Waker) {
        self.lock().waker = Some(waker);
    }

    /// The lines queued since the last call, oldest first. The lock is
    /// released on return, so whatever handles them may log.
    pub(crate) fn drain(&self) -> VecDeque<String> {
        std::mem::take(&mut self.lock().lines)
    }

    /// The writer of one event.
    pub(crate) fn writer(&self) -> Entry {
        Entry {
            tail: self.clone(),
            bytes: Vec::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn push(&self, event: &[u8]) {
        let mut queue = self.lock();
        let Queue { waker, lines } = &mut *queue;
        let Some(waker) = waker else {
            return;
        };
        read_log(event, |line| {
            if lines.len() == LOG_LINES {
                lines.pop_front();
            }
            lines.push_back(line);
            true
        });
        waker.wake();
    }
}

/// One event: the fmt layer makes a writer per event and writes the whole
/// formatted event to it, which is handed on when the writer is dropped.
pub(crate) struct Entry {
    tail: Tail,
    bytes: Vec<u8>,
}

impl Write for Entry {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        // Nowhere to report a failed write; the tail still gets the line.
        let _ = io::stderr().write_all(&self.bytes);
        self.tail.push(&self.bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(tail: &Tail, event: &str) {
        let mut entry = tail.writer();
        entry.write_all(event.as_bytes()).unwrap();
    }

    #[test]
    fn lines_queue_only_once_attached() {
        let tail = Tail::default();
        log(&tail, "prysm-webos-service: info a: before\n");
        assert!(tail.drain().is_empty());
        let wake = prysm_webos_luna_sys::Wake::new().unwrap();
        tail.attach(wake.waker());
        log(&tail, "prysm-webos-service: info a: one\ntwo\n");
        assert_eq!(tail.drain(), ["prysm-webos-service: info a: one", "two"]);
        assert!(tail.drain().is_empty());
    }

    /// Handling a drained line may log: that line waits for the next drain.
    #[test]
    fn logging_while_drained_lines_are_handled_queues_for_the_next_drain() {
        let tail = Tail::default();
        let wake = prysm_webos_luna_sys::Wake::new().unwrap();
        tail.attach(wake.waker());
        log(&tail, "first\n");
        for line in tail.drain() {
            log(&tail, &format!("handled {line}\n"));
        }
        assert_eq!(tail.drain(), ["handled first"]);
    }

    #[test]
    fn a_full_queue_drops_its_oldest_line_and_cuts_long_ones() {
        let tail = Tail::default();
        let wake = prysm_webos_luna_sys::Wake::new().unwrap();
        tail.attach(wake.waker());
        for line in 0..LOG_LINES + 5 {
            log(&tail, &format!("line {line}\n"));
        }
        log(&tail, &format!("{}\n", "x".repeat(4096)));
        let lines = tail.drain();
        assert_eq!(lines.len(), LOG_LINES);
        assert_eq!(lines[0], "line 6");
        assert_eq!(lines[LOG_LINES - 2], format!("line {}", LOG_LINES + 4));
        assert_eq!(lines[LOG_LINES - 1].len(), crate::launcher::MAX_LOG_LINE);
    }
}
