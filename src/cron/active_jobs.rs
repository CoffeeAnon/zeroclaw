//! Registry of in-flight cron job cancellation tokens.
//!
//! The cron scheduler registers a token when it starts an agent job
//! and deregisters it when the job completes. Channel handlers call
//! `cancel_all()` when an interactive message (e.g. Signal) arrives,
//! preempting background cron work so the human gets immediate
//! attention. The cancelled cron's Vikunja task stays [TODO] and
//! gets picked up on the next scheduled fire.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tokio_util::sync::CancellationToken;

type Registry = Arc<Mutex<HashMap<String, CancellationToken>>>;

fn registry() -> &'static Registry {
    static INSTANCE: OnceLock<Registry> = OnceLock::new();
    INSTANCE.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

/// Register a cancellation token for an in-flight cron job.
/// Returns the token (caller passes it to the agent loop).
pub fn register(job_id: &str) -> CancellationToken {
    let token = CancellationToken::new();
    registry()
        .lock()
        .expect("active_jobs lock poisoned")
        .insert(job_id.to_string(), token.clone());
    token
}

/// Remove a completed (or cancelled) job from the registry.
pub fn deregister(job_id: &str) {
    registry()
        .lock()
        .expect("active_jobs lock poisoned")
        .remove(job_id);
}

/// Cancel all in-flight cron jobs. Called when an interactive
/// message arrives and background work should yield.
/// Returns the number of jobs cancelled.
pub fn cancel_all() -> usize {
    let guard = registry().lock().expect("active_jobs lock poisoned");
    let mut count = 0;
    for (_id, token) in guard.iter() {
        if !token.is_cancelled() {
            token.cancel();
            count += 1;
        }
    }
    count
}

/// Return the number of currently registered (in-flight) cron jobs.
pub fn active_count() -> usize {
    registry()
        .lock()
        .expect("active_jobs lock poisoned")
        .len()
}
