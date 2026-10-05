//! The one background owner of blocking teardown.
//!
//! Reaping a worker or a compiler scope takes lifecycle/child locks, scans the process
//! table and waits for a verified exit (seconds in the worst case, unboundedly long
//! while another thread holds one of those locks). None of that may run on the UI thread,
//! so every superseded, rejected or stale object that owns a process (a
//! [`StagedPreview`](crate::preview_coordinator::StagedPreview), build scopes, a queued
//! build specification) is handed to this owner instead of being dropped where it was
//! last used. A hand-off is a bounded, non-blocking queue operation: [`Teardown::submit`]
//! never waits and never runs the work on the caller.
//!
//! Fencing is not delayed by teardown: callers invalidate the object in their own state
//! first (the coordinator clears its build tag under its mailbox lock), so a result from
//! a build being torn down can never be applied while the process is still dying. The
//! outcome of the asynchronous part arrives as a [`TeardownReport`] (only problems are
//! recorded) that the shell drains on its frame loop.
use parking_lot::{Condvar, Mutex};
use std::{
    collections::VecDeque,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, LazyLock,
        atomic::{AtomicU64, Ordering},
        mpsc::{Receiver, SyncSender, TrySendError, sync_channel},
    },
    time::{Duration, Instant},
};
use studio_engine::OperationTag;

/// Jobs the owner thread may have queued; beyond it a job runs on its own short-lived
/// named thread (the submitter still never blocks and never runs the work itself).
pub const QUEUE_CAPACITY: usize = 64;
/// Problem reports retained until the shell drains them (oldest dropped first).
pub const REPORT_CAPACITY: usize = 32;

/// A teardown that did not complete cleanly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeardownReport {
    pub label: &'static str,
    /// The operation the torn-down object belonged to, if it had one.
    pub tag: Option<OperationTag>,
    pub problem: String,
}

struct Job {
    label: &'static str,
    tag: Option<OperationTag>,
    work: Box<dyn FnOnce() -> Option<String> + Send>,
    done: Arc<Done>,
}

#[derive(Default)]
struct Done {
    finished: Mutex<bool>,
    changed: Condvar,
}

/// Completion of one submitted teardown.
#[derive(Clone)]
pub struct Ticket(Arc<Done>);

impl Ticket {
    pub fn is_finished(&self) -> bool {
        *self.0.finished.lock()
    }
    /// Waits for the teardown; `false` on timeout. Never call this on the UI thread.
    pub fn wait(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut finished = self.0.finished.lock();
        while !*finished {
            if self
                .0
                .changed
                .wait_until(&mut finished, deadline)
                .timed_out()
            {
                return *finished;
            }
        }
        true
    }
}

#[derive(Default)]
struct Shared {
    pending: Mutex<usize>,
    idle: Condvar,
    submitted: AtomicU64,
    completed: AtomicU64,
    reports: Mutex<VecDeque<TeardownReport>>,
}

impl Shared {
    fn run(&self, job: Job) {
        let Job {
            label,
            tag,
            work,
            done,
        } = job;
        let problem = match catch_unwind(AssertUnwindSafe(work)) {
            Ok(problem) => problem,
            Err(_) => Some("the teardown work panicked".to_owned()),
        };
        if let Some(problem) = problem {
            let mut reports = self.reports.lock();
            if reports.len() == REPORT_CAPACITY {
                reports.pop_front();
            }
            reports.push_back(TeardownReport {
                label,
                tag,
                problem,
            });
        }
        self.completed.fetch_add(1, Ordering::AcqRel);
        *done.finished.lock() = true;
        done.changed.notify_all();
        let mut pending = self.pending.lock();
        *pending -= 1;
        if *pending == 0 {
            self.idle.notify_all();
        }
    }
}

/// A named teardown thread with a bounded queue.
pub struct Teardown {
    sender: SyncSender<Job>,
    shared: Arc<Shared>,
}

impl Teardown {
    pub fn new(name: &str, capacity: usize) -> Self {
        let (sender, receiver) = sync_channel::<Job>(capacity);
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || owner_loop(receiver, worker))
            .expect("spawn the teardown owner thread");
        Self { sender, shared }
    }

    /// The process-wide owner used by every preview object.
    pub fn global() -> &'static Teardown {
        static GLOBAL: LazyLock<Teardown> =
            LazyLock::new(|| Teardown::new("studio-teardown", QUEUE_CAPACITY));
        &GLOBAL
    }

    /// Queues `work` (which returns a problem description if the teardown could not be
    /// verified). Bounded and non-blocking: never runs on the calling thread and never
    /// waits for queue space.
    pub fn submit(
        &self,
        label: &'static str,
        tag: Option<OperationTag>,
        work: impl FnOnce() -> Option<String> + Send + 'static,
    ) -> Ticket {
        let done = Arc::new(Done::default());
        let job = Job {
            label,
            tag,
            work: Box::new(work),
            done: done.clone(),
        };
        *self.shared.pending.lock() += 1;
        self.shared.submitted.fetch_add(1, Ordering::AcqRel);
        let job = match self.sender.try_send(job) {
            Ok(()) => return Ticket(done),
            Err(TrySendError::Full(job) | TrySendError::Disconnected(job)) => job,
        };
        let shared = self.shared.clone();
        let spill = std::thread::Builder::new()
            .name("studio-teardown-spill".to_owned())
            .spawn({
                let shared = shared.clone();
                move || shared.run(job)
            });
        if spill.is_err() {
            // No thread could be created: the job is dropped unrun (its objects reap
            // themselves on drop) and the failure is reported, never run inline.
            let mut reports = shared.reports.lock();
            if reports.len() == REPORT_CAPACITY {
                reports.pop_front();
            }
            reports.push_back(TeardownReport {
                label,
                tag: None,
                problem: "no thread was available for the teardown".into(),
            });
            drop(reports);
            shared.completed.fetch_add(1, Ordering::AcqRel);
            *done.finished.lock() = true;
            done.changed.notify_all();
            let mut pending = shared.pending.lock();
            *pending -= 1;
            if *pending == 0 {
                shared.idle.notify_all();
            }
        }
        Ticket(done)
    }

    /// Drops `value` on the owner thread. Convenience for objects whose destructor
    /// reaps processes.
    pub fn discard<T: Send + 'static>(&self, label: &'static str, value: T) -> Ticket {
        self.submit(label, None, move || {
            drop(value);
            None
        })
    }

    /// Teardowns queued or running.
    pub fn pending(&self) -> usize {
        *self.shared.pending.lock()
    }
    pub fn submitted(&self) -> u64 {
        self.shared.submitted.load(Ordering::Acquire)
    }
    pub fn completed(&self) -> u64 {
        self.shared.completed.load(Ordering::Acquire)
    }
    /// Waits until nothing is pending; `false` on timeout. Not for the UI thread.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut pending = self.shared.pending.lock();
        while *pending > 0 {
            if self
                .shared
                .idle
                .wait_until(&mut pending, deadline)
                .timed_out()
            {
                return *pending == 0;
            }
        }
        true
    }
    /// Problems since the last call (oldest first).
    pub fn take_reports(&self) -> Vec<TeardownReport> {
        self.shared.reports.lock().drain(..).collect()
    }
}

fn owner_loop(receiver: Receiver<Job>, shared: Arc<Shared>) {
    while let Ok(job) = receiver.recv() {
        shared.run(job);
    }
}

/// Drops `value` on the process-wide teardown owner.
pub fn discard<T: Send + 'static>(label: &'static str, value: T) {
    Teardown::global().discard(label, value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Barrier, atomic::AtomicBool};

    #[test]
    fn submit_never_runs_on_the_caller_and_never_blocks_behind_a_stuck_job() {
        let owner = Teardown::new("test-teardown", 2);
        let release = Arc::new(Barrier::new(2));
        let caller = std::thread::current().id();
        let gate = release.clone();
        let stuck = owner.submit("stuck", None, move || {
            gate.wait();
            None
        });
        let ran_elsewhere = Arc::new(AtomicBool::new(false));
        let started = Instant::now();
        // More jobs than the queue holds: the surplus spills to named threads.
        let tickets: Vec<_> = (0..6)
            .map(|_| {
                let flag = ran_elsewhere.clone();
                owner.submit("quick", None, move || {
                    flag.store(std::thread::current().id() != caller, Ordering::SeqCst);
                    None
                })
            })
            .collect();
        assert!(started.elapsed() < Duration::from_millis(200));
        assert!(!stuck.is_finished());
        // Spilled jobs complete even while the owner thread is stuck on the first job.
        assert!(owner.pending() >= 1);
        release.wait();
        assert!(stuck.wait(Duration::from_secs(5)));
        for ticket in tickets {
            assert!(ticket.wait(Duration::from_secs(5)));
        }
        assert!(ran_elsewhere.load(Ordering::SeqCst));
        assert!(owner.wait_idle(Duration::from_secs(5)));
        assert_eq!(owner.submitted(), owner.completed());
        assert!(owner.take_reports().is_empty());
    }

    #[test]
    fn problems_and_panics_are_reported_with_their_label() {
        let owner = Teardown::new("test-teardown-reports", 4);
        owner
            .submit("unverified", None, || Some("a child survived".into()))
            .wait(Duration::from_secs(5));
        owner
            .submit("panicking", None, || panic!("boom"))
            .wait(Duration::from_secs(5));
        assert!(owner.wait_idle(Duration::from_secs(5)));
        let reports = owner.take_reports();
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].label, "unverified");
        assert_eq!(reports[0].problem, "a child survived");
        assert_eq!(reports[1].label, "panicking");
        assert!(owner.take_reports().is_empty());
    }
}
