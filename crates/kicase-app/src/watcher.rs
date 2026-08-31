//! Watching the board for changes.
//!
//! KiCad's IPC API has no events — all 59 commands are request/response — so
//! noticing that you moved something means asking. The asking happens on its
//! own thread so the window never stalls on a round trip, and it only reports
//! *changes*: the board document is hashed, and the UI is told when the hash
//! moves.

use kicase_kicad::client::KiCadSession;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

/// How often the board is checked while nothing is happening.
///
/// Serialising a whole board four times a second forever is the price of KiCad
/// having no change events, so an idle project pays as little of it as it can
/// get away with.
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// How often it is checked just after a change.
///
/// Someone editing makes several changes in a row, and once the rest of an
/// update is measured in tens of milliseconds the poll is most of the wait. So
/// the watcher leans in while the user is working and settles back when they
/// stop.
const ACTIVE_INTERVAL: Duration = Duration::from_millis(60);

/// How long a change keeps the watcher leaning in.
const ACTIVE_FOR: Duration = Duration::from_secs(3);

/// Where the watcher reads the board from.
pub enum WatchSource {
    /// A running KiCad, over its own IPC connection.
    LiveKiCad,
    /// A saved board file.
    File(PathBuf),
}

/// A background board watcher. Dropping it stops the thread.
pub struct BoardWatcher {
    changes: Receiver<()>,
    stop: Arc<AtomicBool>,
}

/// Called from the watcher thread when the board changes, so the window can
/// sleep until there is something to do rather than waking to ask.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

impl Drop for BoardWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl BoardWatcher {
    /// Starts watching. The thread owns its own connection to KiCad, so it
    /// never contends with the one the window is using.
    pub fn start(source: WatchSource, waker: Option<Waker>) -> Self {
        let (sender, changes) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();

        std::thread::spawn(move || {
            let mut session = None;
            let mut last: Option<u64> = None;
            let mut last_change: Option<std::time::Instant> = None;

            while !thread_stop.load(Ordering::Relaxed) {
                let text = match &source {
                    WatchSource::File(path) => std::fs::read_to_string(path).ok(),
                    WatchSource::LiveKiCad => {
                        if session.is_none() {
                            session = KiCadSession::connect().ok();
                        }
                        session.as_ref().and_then(|s| s.board_text().ok())
                    },
                };

                if let Some(text) = text {
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    text.hash(&mut hasher);
                    let hash = hasher.finish();
                    let changed = last.is_some_and(|previous| previous != hash);
                    last = Some(hash);
                    if changed {
                        last_change = Some(std::time::Instant::now());
                        if sender.send(()).is_err() {
                            // The window has gone; nothing left to tell.
                            return;
                        }
                        if let Some(waker) = &waker {
                            waker();
                        }
                    }
                } else if matches!(source, WatchSource::LiveKiCad) {
                    // KiCad went away: drop the connection and try again later.
                    session = None;
                }

                let active = last_change.is_some_and(|at| at.elapsed() < ACTIVE_FOR);
                std::thread::sleep(if active { ACTIVE_INTERVAL } else { POLL_INTERVAL });
            }
        });

        BoardWatcher { changes, stop }
    }

    /// True when the board has changed since the last call.
    ///
    /// Several changes collapse into one: what matters is that a rebuild is due,
    /// not how many edits happened.
    pub fn take_change(&self) -> bool {
        let mut changed = false;
        while self.changes.try_recv().is_ok() {
            changed = true;
        }
        changed
    }
}
