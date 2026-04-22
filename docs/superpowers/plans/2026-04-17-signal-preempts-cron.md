# Signal Preempts Cron Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When a Signal message arrives from Dan while Sam is running an isolated cron session, cancel the cron session so the Signal message gets immediate attention. The cron's Vikunja task stays [TODO] and gets picked up on the next fire — interruption is safe because the task queue is durable.

**Architecture:** Thread `tokio_util::sync::CancellationToken` from the cron scheduler through `agent::run()` to `run_tool_call_loop()`, where it's already checked between tool calls (line 1287). Store active cron tokens in a shared registry. When a Signal message arrives in `channels/mod.rs`, cancel any active cron token. The cron scheduler catches `ToolLoopCancelled` and logs "preempted" instead of "failed."

**Tech Stack:** Rust, tokio, tokio_util::sync::CancellationToken (already a dependency), zeroclaw agent runtime.

---

## Background

### Why this is safe

Sam's task queue lives in Vikunja. A cancelled cron session leaves the task [TODO]. The `task-queue-check` cron fires every 30 min and picks it back up. Preemption is a no-op from the task system's perspective — the work is durable, only the session is ephemeral.

### What already exists

- **CancellationToken in the agent loop** (`src/agent/loop_.rs:1287`): `run_tool_call_loop` accepts `cancellation_token: Option<CancellationToken>` and checks `is_cancelled()` at the top of every tool-call iteration. Returns `ToolLoopCancelled` error.
- **Mid-turn injection** (`src/channels/mod.rs:4807-4813`): Signal messages arriving during an active session get queued in per-sender injection channels. This is where we intercept.
- **`agent::run()` already has extensible params** (`src/agent/loop_.rs:2852`): we added `session_id: Option<String>` in v1.5.15; the cancellation token goes alongside it.

### What doesn't exist yet

- Cron jobs don't create or pass a CancellationToken.
- No shared registry mapping active cron jobs to their tokens.
- No code in the channel handler to cancel cron tokens on Signal message arrival.
- `agent::run()` doesn't accept or forward a CancellationToken to the inner loop.

---

## File Structure

- **Create** `src/cron/active_jobs.rs` — shared registry of active cron CancellationTokens. Small module (~40 lines): a `OnceLock<Arc<Mutex<HashMap<String, CancellationToken>>>>` with `register`, `deregister`, and `cancel_all` functions.
- **Modify** `src/cron/scheduler.rs` — create token per cron job, register it, pass it through `agent::run()`, deregister on completion, handle `ToolLoopCancelled` as "preempted."
- **Modify** `src/cron/mod.rs` — add `pub mod active_jobs;` declaration.
- **Modify** `src/agent/loop_.rs` — add `cancellation_token: Option<CancellationToken>` parameter to `agent::run()`, thread it to the inner `run_tool_call_loop` call.
- **Modify** `src/channels/mod.rs` — on Signal message arrival (in the injection path), call `active_jobs::cancel_all()` to preempt any running cron.
- **Modify** `src/daemon/mod.rs` — pass `None` for the new token param (heartbeat, no preemption).
- **Modify** `src/main.rs` — pass `None` for the new token param (CLI, no preemption).

---

### Task 1: Create the active-jobs registry

**Files:**
- Create: `src/cron/active_jobs.rs`
- Modify: `src/cron/mod.rs`

The registry is a global map from job ID to CancellationToken. It must be accessible from both the cron scheduler (register/deregister) and the channel handler (cancel). A `OnceLock<Arc<Mutex<HashMap>>>` is the simplest pattern that works across async contexts without lifetime gymnastics.

- [ ] **Step 1: Create `src/cron/active_jobs.rs`**

```rust
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
```

- [ ] **Step 2: Add the module declaration in `src/cron/mod.rs`**

Find the existing `pub mod scheduler;` line and add:

```rust
pub mod active_jobs;
```

- [ ] **Step 3: Verify it compiles**

```bash
cd /home/wsl2user/github_projects/zeroclaw
cargo check --bin zeroclaw 2>&1 | tail -5
```

Expected: no errors (the module is defined but not yet called).

- [ ] **Step 4: Commit**

```bash
git add src/cron/active_jobs.rs src/cron/mod.rs
git commit -m "feat(cron): add active-jobs cancellation token registry"
```

---

### Task 2: Thread CancellationToken through `agent::run()`

**Files:**
- Modify: `src/agent/loop_.rs`
- Modify: `src/daemon/mod.rs`
- Modify: `src/main.rs`

`agent::run()` already has a `session_id: Option<String>` parameter we added in v1.5.15. Add `cancellation_token: Option<CancellationToken>` alongside it and forward it to the inner `run_tool_call_loop` call.

- [ ] **Step 1: Read the current `agent::run()` signature**

```bash
grep -n 'pub async fn run(' src/agent/loop_.rs | head -5
```

Note the line number and the current parameter list. The last param should be `session_id: Option<String>`.

- [ ] **Step 2: Add the cancellation_token parameter to `agent::run()`**

At line ~2852 (wherever `pub async fn run(` is), add `cancellation_token: Option<CancellationToken>` as the final parameter:

```rust
pub async fn run(
    config: Config,
    message: Option<String>,
    provider_override: Option<String>,
    model_override: Option<String>,
    temperature: f64,
    peripheral_overrides: Vec<String>,
    interactive: bool,
    hooks: Option<&crate::hooks::HookRunner>,
    session_id: Option<String>,
    cancellation_token: Option<CancellationToken>,  // NEW
) -> Result<String> {
```

Add the import if not already present:

```rust
use tokio_util::sync::CancellationToken;
```

- [ ] **Step 3: Find where `run()` calls `run_tool_call_loop` and forward the token**

Search inside `run()` for the call to `run_tool_call_loop` (or whichever inner function accepts `cancellation_token`). It's currently passing `None` (or not passing it at all if the param is optional with a default). Change it to pass the new parameter:

Before:
```rust
// somewhere inside run(), the call to the tool loop
// look for: cancellation_token: None
```

After:
```rust
// pass through the caller's token
// cancellation_token: cancellation_token.clone()  (or cancellation_token)
```

**IMPORTANT:** Read the actual code to find the exact call site. The inner function might be `run_tool_call_loop`, `run_tool_call_loop_with_reply_target`, or similar. Search for `cancellation_token` inside `run()` to find where `None` is currently hardcoded.

- [ ] **Step 4: Update all callers of `agent::run()` to pass `None`**

There are 3 callers (from the v1.5.15 session_id change):

**`src/cron/scheduler.rs`** (~line 233-244):
Add `None` as the last argument (we'll change this to a real token in Task 3):
```rust
cancellation_token: None,  // Task 3 replaces with real token
```

Wait — actually, we should add the real token here in Task 3. For now, pass `None` so it compiles.

**`src/daemon/mod.rs`** (~line 261):
Add `None` as the last argument:
```rust
None, // cancellation_token — heartbeat tasks, no preemption
```

**`src/main.rs`** (~line 976):
Add `None` as the last argument:
```rust
None, // cancellation_token — CLI, no preemption
```

- [ ] **Step 5: Verify it compiles**

```bash
cargo check --bin zeroclaw 2>&1 | tail -5
```

Expected: clean (all callers updated, token forwarded).

- [ ] **Step 6: Commit**

```bash
git add src/agent/loop_.rs src/daemon/mod.rs src/main.rs src/cron/scheduler.rs
git commit -m "feat(agent): thread CancellationToken through agent::run()"
```

---

### Task 3: Wire the token in the cron scheduler

**Files:**
- Modify: `src/cron/scheduler.rs`

Replace the `None` cancellation token in `run_agent_job()` with a real token from the active-jobs registry. Register before the agent call, deregister after (in all exit paths). Handle `ToolLoopCancelled` as a preemption, not a failure.

- [ ] **Step 1: Read the current `run_agent_job()` function**

```bash
grep -n -A 30 'async fn run_agent_job' src/cron/scheduler.rs
```

Note:
- Where `agent::run()` is called
- How the result is matched (`Ok(response)` / `Err(e)`)
- The `session_id` generation for Isolated jobs (our v1.5.15 code)

- [ ] **Step 2: Add imports**

At the top of `scheduler.rs`:

```rust
use crate::cron::active_jobs;
use tokio_util::sync::CancellationToken;
```

- [ ] **Step 3: Create and register the token before calling `agent::run()`**

Find the section where `session_id` is generated (our v1.5.15 code). Right after it, add:

```rust
// Register a cancellation token so Signal messages can preempt
// this cron session. The task-queue system is durable (Vikunja),
// so cancellation just means "retry on next cron fire."
let cancellation_token = active_jobs::register(&job.id);
```

- [ ] **Step 4: Pass the token to `agent::run()`**

In the `agent::run()` call, replace the `None` cancellation_token argument with:

```rust
Some(cancellation_token.clone()),
```

- [ ] **Step 5: Deregister on completion (all exit paths)**

After the `agent::run()` call returns (both Ok and Err paths), deregister:

```rust
let run_result = Box::pin(crate::agent::run(
    // ... existing args ...
    session_id,
    Some(cancellation_token.clone()),
))
.await;

// Always deregister, whether the job succeeded, failed, or was cancelled
active_jobs::deregister(&job.id);
```

- [ ] **Step 6: Handle preemption in the result match**

Find the `match run_result` block. Add a specific arm (or check inside `Err`) for cancellation. The error type from `run_tool_call_loop` when cancelled is likely an `anyhow::Error` wrapping a string like "ToolLoopCancelled" or similar. Read the error path in `loop_.rs` to find the exact error message/type.

```rust
match run_result {
    Ok(response) => (
        true,
        if response.trim().is_empty() {
            "agent job executed".to_string()
        } else {
            response
        },
    ),
    Err(e) => {
        let msg = format!("{e}");
        if msg.contains("cancelled") || msg.contains("Cancelled") {
            tracing::info!(
                "Cron job '{}' ({}) preempted by interactive message",
                name, job.id
            );
            (true, "preempted by interactive message".to_string())
        } else {
            (false, format!("agent job failed: {e}"))
        }
    }
}
```

**IMPORTANT:** The exact error string depends on how `run_tool_call_loop` surfaces the cancellation. Read the code at `loop_.rs:1287-1290` to find the exact error message or type. Adjust the string match accordingly.

- [ ] **Step 7: Verify it compiles**

```bash
cargo check --bin zeroclaw 2>&1 | tail -5
```

- [ ] **Step 8: Commit**

```bash
git add src/cron/scheduler.rs
git commit -m "feat(cron): register cancellation token and handle preemption"
```

---

### Task 4: Cancel cron tokens on Signal message arrival

**Files:**
- Modify: `src/channels/mod.rs`

This is the trigger: when a Signal message arrives, cancel all active cron tokens. The agent loop will check `is_cancelled()` at its next tool-call boundary and exit.

- [ ] **Step 1: Find the Signal message dispatch path**

Search for the mid-turn injection log line we observed:

```bash
grep -n 'mid-turn injection' src/channels/mod.rs | head -5
```

Also find the main message dispatch (non-injection path — when no loop is active for the sender):

```bash
grep -n 'Processing message\|process_message\|handle_message' src/channels/mod.rs | head -10
```

- [ ] **Step 2: Add the import**

Near the top of `mod.rs`:

```rust
use crate::cron::active_jobs;
```

- [ ] **Step 3: Add cancellation call on Signal message arrival**

There are two paths a Signal message can take:

**Path A — No active loop for this sender (new message):**
Before the message gets dispatched to a new agent session, cancel any active crons:

```rust
// Signal message arrived — preempt any background cron work
// so the human gets immediate attention.
let cancelled = active_jobs::cancel_all();
if cancelled > 0 {
    tracing::info!(
        "Preempted {cancelled} cron job(s) for incoming Signal message"
    );
}
```

**Path B — Active loop for this sender (mid-turn injection):**
At the injection point (where the log says "mid-turn injection: queued message for active loop"), ALSO cancel cron tokens. Even though the message is being queued for an active Signal loop (not a cron), cancelling crons ensures the Signal session gets the GPU slot:

```rust
// Also cancel crons even on injection path — free up GPU slots
// for the interactive session
let cancelled = active_jobs::cancel_all();
if cancelled > 0 {
    tracing::info!(
        "Preempted {cancelled} cron job(s) for Signal injection"
    );
}
```

**IMPORTANT:** Only cancel for **Signal** messages (interactive human channel), NOT for ACP, webhook, or other programmatic channels. Check what channel identifier is available at the call site and gate on it. Look for a `channel` or `channel_name` field — it should be `"signal"` for Signal messages.

- [ ] **Step 4: Verify it compiles**

```bash
cargo check --bin zeroclaw 2>&1 | tail -5
```

- [ ] **Step 5: Run existing tests**

```bash
cargo test --bin zeroclaw --lib -- 2>&1 | tail -10
```

Expected: existing tests pass (our change is additive — `cancel_all()` on an empty registry is a no-op).

- [ ] **Step 6: Commit**

```bash
git add src/channels/mod.rs
git commit -m "feat(channels): cancel active crons on Signal message arrival

When a Signal message arrives (interactive human channel), cancel
all in-flight cron agent sessions via the active_jobs registry.
The agent loop checks is_cancelled() between tool calls and exits
gracefully. The cron scheduler logs 'preempted' instead of 'failed',
and the Vikunja task stays [TODO] for the next cron fire.

Only triggers for Signal channel — ACP, webhook, and other
programmatic channels do not preempt crons."
```

---

### Task 5: Build, test, and deploy

**Files:**
- Modify: `k8s/sam/04_zeroclaw_sandbox.yaml` (image tag bump)

- [ ] **Step 1: Full build**

```bash
cd /home/wsl2user/github_projects/zeroclaw
cargo build --bin zeroclaw 2>&1 | tail -10
```

Expected: compiles with 0 errors. Warnings about unused mut in signal.rs are pre-existing and OK.

- [ ] **Step 2: Run tests**

```bash
cargo test --bin zeroclaw --lib 2>&1 | tail -10
```

Expected: all tests pass (186+ tests, 0 failures).

- [ ] **Step 3: Build Docker image**

```bash
DOCKER_BUILDKIT=1 docker build -f Dockerfile.sam \
  -t citizendaniel/zeroclaw-sam:v1.5.16 . 2>&1 | tail -5
```

- [ ] **Step 4: Push**

```bash
docker push citizendaniel/zeroclaw-sam:v1.5.16 2>&1 | tail -5
```

- [ ] **Step 5: Bump manifest**

In `k8s/sam/04_zeroclaw_sandbox.yaml`, change:
```yaml
image: citizendaniel/zeroclaw-sam:v1.5.15
```
to:
```yaml
image: citizendaniel/zeroclaw-sam:v1.5.16
```

Use `git add -p` to stage only the image tag hunk (same dirty-file handling as previous deploys).

- [ ] **Step 6: Apply and roll Sam's pod**

```bash
# Stash dirty science-curator hunks, apply clean, unstash
cd /home/wsl2user/github_projects/zeroclaw
git stash push k8s/sam/04_zeroclaw_sandbox.yaml > /dev/null 2>&1
kubectl apply -f k8s/sam/04_zeroclaw_sandbox.yaml
git stash pop > /dev/null 2>&1
kubectl delete pod zeroclaw -n ai-agents
```

Wait for `2/2 Running`.

- [ ] **Step 7: Also bump and roll Walter**

Walter runs the same binary. His manifest is at `k8s/walter/03_sandbox.yaml`. Bump both image occurrences to `v1.5.16` and apply + roll.

Walter doesn't benefit from Signal preemption (he has no Signal channel), but keeping him on the same binary version avoids drift.

- [ ] **Step 8: Commit the manifest changes**

```bash
git add -p k8s/sam/04_zeroclaw_sandbox.yaml  # only image tag hunk
git add k8s/walter/03_sandbox.yaml
git commit -m "chore: bump Sam + Walter to v1.5.16 (Signal preempts cron)"
```

---

### Task 6: End-to-end verification

**Files:** None modified.

- [ ] **Step 1: Trigger a cron manually**

Wait for Sam's pod to be `2/2 Running`, then trigger the `task-queue-check` cron:

```bash
kubectl exec -n ai-agents zeroclaw -c zeroclaw -- python3 -c "
import sqlite3
c = sqlite3.connect('/data/.zeroclaw/workspace/cron/jobs.db')
row = c.execute(\"SELECT id FROM cron_jobs WHERE name='task-queue-check'\").fetchone()
print(row[0])
"
```

Ask Dan to trigger it via Signal: *"Run the task-queue-check cron please"* (or use `cron_run` if available).

Alternatively, wait for the next scheduled fire (every 30 min at :00/:30).

- [ ] **Step 2: While the cron is running, send a Signal message**

While Sam is mid-cron (visible in logs as tool calls for vikunja my-tasks, file operations, etc.), Dan sends a Signal message:

> Hey Sam, quick question — what's the weather like in your pod?

- [ ] **Step 3: Check the logs for preemption**

```bash
kubectl logs -n ai-agents zeroclaw -c zeroclaw --since=5m | grep -iE 'preempt|cancel|Signal|cron.*job'
```

Expected:
- `"Preempted 1 cron job(s) for incoming Signal message"` — from channels/mod.rs
- `"Cron job '...' (...) preempted by interactive message"` — from scheduler.rs
- Sam's reply to Dan's weather question (NOT a continuation of the cron work)

- [ ] **Step 4: Verify the cron's task is still open**

```bash
kubectl exec -n ai-agents zeroclaw -c zeroclaw -- vikunja my-tasks --open
```

The task the cron was working on should still be [TODO] (not [DONE] — it was interrupted).

- [ ] **Step 5: Wait for the next cron fire and verify it picks the task back up**

On the next :00 or :30, the cron should fire again, pick up the same task, and complete it.

---

### Task 7: Documentation

**Files:**
- Modify: `~/github_projects/scrapyard-wiki/wiki/services/zeroclaw.md`

- [ ] **Step 1: Add a "Signal Preempts Cron" subsection**

Under Custom Fork Changes, add an entry documenting:
- The feature: Signal messages cancel active cron sessions via CancellationToken
- The mechanism: active_jobs registry + token threading + channel-level cancel_all
- Why it's safe: Vikunja task queue is durable; cancelled crons retry on next fire
- The files changed
- The version: v1.5.16

- [ ] **Step 2: Update image version history**

Add a v1.5.16 row to the Image Version History table.

- [ ] **Step 3: Commit**

```bash
cd ~/github_projects/scrapyard-wiki
git add wiki/services/zeroclaw.md
git commit -m "wiki: document Signal preempts cron (v1.5.16)"
```

---

## Self-review checklist

- [ ] `active_jobs::register` and `deregister` are called in matching pairs (no token leak on error paths)
- [ ] `cancel_all()` only fires for Signal channel, not ACP/webhook
- [ ] `agent::run()` signature is consistent across all 3 callers + the definition
- [ ] The cancellation error string match in scheduler.rs matches the actual error from `run_tool_call_loop`
- [ ] Walter's image is bumped alongside Sam's (same binary)
- [ ] No pre-existing dirty files (science-curator) accidentally committed

## Non-goals

- Saving cron progress to Vikunja before cancellation (Option C from the design discussion — deferred; the task just stays [TODO])
- Preempting crons for ACP messages (Walter→Sam) — only interactive Signal triggers preemption
- Preempting one cron for another cron (crons are peers; only humans preempt)
- Rate-limiting preemption (a burst of Signal messages cancels once; the registry clears after deregister)
