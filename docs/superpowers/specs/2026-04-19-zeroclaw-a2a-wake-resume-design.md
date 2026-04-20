# ZeroClaw A2A Wake/Resume Layer — Design

**Date:** 2026-04-19
**Status:** Design approved — ready for implementation plan
**Scope:** MVP — inter-agent wake-up (Walter → Sam) via Google A2A v1.0 push notifications

## Problem

ZeroClaw's agent loop has no subscribe/wake mechanism. When Sam delegates a task to Walter via ACP, Walter can respond synchronously to the request, but has no way to push unsolicited updates back to Sam once the synchronous call returns. Today, Walter leaves findings as a Vikunja task comment and Sam picks them up on her next 30-minute poll. This makes multi-agent coordination feel sluggish and limits how Sam can react to asynchronous events.

The architectural fix is to separate the **agent** (durable state + identity) from the **loop** (stateless worker that runs one step at a time). Once the agent state is externalized, any event — inter-agent, external webhook, scheduled trigger — can wake the worker to run the next step.

## Goals

- Walter can push a completion notification to Sam, and Sam's reasoning loop wakes immediately to process it (not on next poll).
- The pattern is aligned with Google's A2A v1.0 spec so future LangGraph augmentation and inter-agent interop with other frameworks are low-friction.
- Existing ACP, Signal, cron, and Vikunja paths continue to work unchanged during and after rollout.
- Failure modes (either agent offline, transient network, token mismatch) are handled with durable retries, not lost messages.

## Non-goals (MVP)

- External webhooks (GitHub, Gitea, Grafana) — same server can host these later; not in scope now.
- Migrating Signal / cron / Vikunja triggers to the new wake path — they stay on polling.
- Replacing ACP for external callers (Dan's CLI, etc.) — ACP remains the external contract.
- Streaming (SSE) of intermediate artifacts from Walter to Sam — `ra2a` supports it; no current consumer needs it.
- Mid-execute crash resumption on Walter — MVP handles by emitting `Task{state:failed}` on pod startup scan. True resume-from-checkpoint is deferred.
- LangGraph integration — explicitly post-MVP; A2A compliance is the bridge.

## Approach

Adopt the [`ra2a`](https://github.com/qntx/ra2a) Rust crate (v0.10.x, Apache-2.0, A2A v1.0 compliant) to provide the A2A server + client layer. Both Sam and Walter gain an A2A server alongside their existing ACP handlers. Sam's delegation to Walter uses A2A's `message/send` with a `pushNotificationConfig` pointing at Sam's own webhook. Walter's completion POSTs a `statusUpdate` to that webhook. Sam's webhook handler enqueues a wake on her existing reasoning-loop queue.

Durability follows A2A's design: **outboxes on the sending side**, not inboxes on the receiver. Both agents get a Postgres-backed outbox table and a worker task that retries with exponential backoff and dead-letters after N attempts.

### `ra2a` assessment

| Signal | Value |
|---|---|
| License | Apache-2.0 |
| Version | v0.10.1 on crates.io, bumped 2026-04-08 |
| Last commit | 2026-04-11 (8 days before this design) |
| A2A spec | v1.0 compliant (spec released 2026-03-12) |
| Stars / forks | 162 / 26 |
| Open issues | 1 |
| Maintainer | Single (`gitctrlx`, 92 commits). Bus factor of 1. |
| Downloads | 859 total |

Pre-1.0 + single-maintainer is a yellow flag. Mitigation: Apache-licensed, vendorable if the project stalls. Alternative (implementing A2A v1.0 from scratch in Rust) is multi-week work that `ra2a` already has behind it.

## Architecture

Both Sam and Walter gain the `ra2a` server layer alongside their existing ACP handlers. No existing paths are removed in MVP.

```
Sam (caller)                Walter (callee)
────────────                ──────────────
1. POST /a2a/message:send  ─────────────▶
   with pushNotificationConfig
   (URL = Sam's own webhook)

2.                           Returns Task {id, state: "submitted"}
                             ◀───────────

3.                           Walter's AgentExecutor runs
                             (kubectl observations, reporting)

4.                           Walter POSTs statusUpdate
   /webhook/a2a-notify       ◀───────────  to Sam's webhook
   (Bearer auth)

5. Sam's webhook handler
   writes to turn_queue →
   reasoning loop wakes
   and processes the result
```

Both agents expose an AgentCard at `/.well-known/agent-card.json` declaring their capabilities (streaming, push_notifications).

## Components

### Shared (both agents)

- `ra2a` crate dependency
- `a2a_router` composed into the existing Axum server on port 3000
- `AgentCard` static definition served at `/.well-known/agent-card.json`
- Postgres `TaskStore` and `PushNotificationConfigStore` using `ra2a`'s built-in Postgres backend
- New crate `crates/zeroclaw-a2a-outbox`
  - Table: `outbox(id, target_url, payload_json, attempts, next_attempt_at, status, created_at, last_error)`
  - Worker: polls `WHERE status='pending' AND next_attempt_at <= now()`, POSTs, updates status
  - Retry: exponential backoff (1s, 4s, 16s, 64s, 256s), max 5 attempts
  - Dead-letter: `status='deadletter'` after max attempts; emits a structured log event
- Custom `OutboxBackedPushSender` — our own impl of `ra2a`'s `PushNotificationSender` trait. Instead of POSTing directly (as `ra2a::HttpPushSender` does), it inserts a row into `outbox` and returns immediately. Decouples `ra2a`'s task state updates from actual network delivery, and gives us durable retries for free. The outbox worker is what actually hits the network.

### Walter-specific

- `WalterAgentExecutor` (impl `ra2a::AgentExecutor`)
  - Receives `message/send` requests
  - Runs the read-only observation workflow (same kubectl path as today)
  - Emits `TaskArtifactUpdate` events for streaming findings (optional, off by default)
  - Final `Task{state:completed}` with full report as the terminal artifact
- Startup scan: on boot, look for `Task{state:working}` rows belonging to this agent; emit `Task{state:failed}` with reason `"walter restarted mid-execute"` → triggers outbox delivery to the original caller

### Sam-specific

- `SamAgentExecutor` (impl `ra2a::AgentExecutor`) — thin shim in MVP; no A2A clients call Sam yet, but the server is present and the executor exists for symmetry and future use.
- `WebhookReceiver` — new Axum handler at `POST /webhook/a2a-notify`
  - Verifies `Authorization: Bearer <token>` against the stored `push_notification_config.token` for the `taskId`
  - Writes `inbox_events` row (idempotent on `(task_id, sequence)`)
  - Enqueues a wake on Sam's existing reasoning-loop queue
  - Returns 200 on success; 401 on token mismatch (causes caller to dead-letter — intentional); 5xx on transient errors
- Change to Sam's existing delegation code: instead of direct ACP call to Walter, uses `ra2a::Client::send_message` with `PushNotificationConfig` pointing at Sam's own webhook URL. Gated by env var `ZEROCLAW_USE_A2A_DELEGATION` (see Phase 2 in Migration).

### Infrastructure (Kubernetes side)

- New CNPG `Cluster` named `zeroclaw-a2a` in `ai-agents` namespace
  - Single database `a2a`
  - Schemas: `sam`, `walter`
  - Role per agent, `GRANT USAGE, CREATE` on its own schema only
- `VaultStaticSecret` per agent for DB credentials (Vault → VSO → Secret → env var)
- Push notification tokens: `ra2a` generates per-task; stored in `push_notification_configs` table
- Sam's webhook URL: `http://zeroclaw.ai-agents.svc.cluster.local:3000/webhook/a2a-notify` (in-cluster DNS; no external exposure)

## Data flow — end-to-end sequence

Happy path: Sam delegates a cluster health check to Walter.

| Step | Actor | Action | Persisted where |
|---|---|---|---|
| 1 | Sam loop | Decides to delegate | (in-memory) |
| 2 | Sam | Writes row to `sam.outbox` with target URL, payload, `task_id`, push config | Postgres `sam` schema (txn) |
| 3 | Sam outbox worker | POSTs `message/send` to Walter | — |
| 4 | Walter | `ra2a` handler writes `Task{state:submitted}` to `walter.tasks` + stores push config in `walter.push_notification_configs` | Postgres `walter` schema (txn) |
| 5 | Walter | Returns 200 + Task envelope | — |
| 6 | Sam outbox worker | Marks row delivered, records `task_id` | Postgres `sam` schema |
| 7 | Walter | `AgentExecutor::execute` runs on the `EventQueue`; kubectl work | — |
| 8 | Walter | Emits final `Task{state:completed}` with artifacts; `ra2a` updates `walter.tasks` | Postgres `walter` schema (txn) |
| 9 | Walter | `OutboxBackedPushSender` inserts `walter.outbox` row with statusUpdate payload | Postgres `walter` schema (ideally same txn as 8; see note) |
| 10 | Walter outbox worker | POSTs `statusUpdate` to Sam's `/webhook/a2a-notify` with bearer token | — |
| 11 | Sam webhook handler | Verifies token, writes `sam.inbox_events`, enqueues wake | Postgres `sam` schema (txn) |
| 12 | Sam webhook handler | Returns 200 | — |
| 13 | Walter outbox worker | Marks delivered | Postgres `walter` schema |
| 14 | Sam reasoning loop | Dequeues wake, reads `inbox_events` + task data, resumes turn | — |

**Transactionality note for step 9:** Atomically coupling the `walter.tasks` update (done by `ra2a`) with the `walter.outbox` insert (done by `OutboxBackedPushSender`) depends on whether `ra2a` exposes a hook that runs inside its task-update transaction. If it does, we wire it in. If it doesn't, we accept eventual consistency — a reconciliation worker scans for terminal tasks that lack a corresponding outbox row and emits the missing statusUpdate. Plan and code review during implementation will settle which path is possible.

## Wake integration with Sam's reasoning loop

The webhook handler's job is to: (a) validate the statusUpdate, (b) persist it durably, (c) cause Sam's reasoning loop to run a new turn that processes the update.

ZeroClaw's current loop structure needs to be inspected during implementation to decide the exact mechanism:

- **If Sam already has an internal event/trigger queue** (similar to how Signal messages or cron firings dispatch turns), the webhook handler enqueues on that.
- **If not**, the implementation plan adds a lightweight trigger channel (`tokio::sync::mpsc` or equivalent) that the main loop selects on alongside its existing trigger sources.

Either way, the contract is: webhook handler writes to `sam.inbox_events`, signals the loop via whatever channel it uses, and returns 200. The loop's next iteration reads the inbox events and runs a turn.

## Failure modes

- **Walter unreachable (step 3).** Sam's outbox worker retries with backoff. After max attempts, dead-letters → Sam's loop receives a synthetic error event and can mark the delegation as failed.
- **Sam unreachable (step 10).** Walter's outbox holds the statusUpdate; retries with backoff. Durability on Walter's side means Sam can be offline for hours and still get the update when she's back.
- **Webhook token mismatch (step 11).** 401. Walter treats 4xx as non-retryable → immediate dead-letter, logged as config error.
- **5xx from webhook.** Retryable via outbox backoff.
- **DB write fails at any step.** Transaction rolls back; HTTP handler returns 5xx; caller retries via its outbox.
- **Walter crashes mid-execute (between steps 7 and 8).** `ra2a`'s task store still has `Task{state:working}`. On Walter's next boot, startup scan finds the stuck row and emits `Task{state:failed}` with reason `"walter restarted mid-execute"`. That enqueues an outbox delivery to Sam so she can decide whether to retry. True resume-from-checkpoint is out of scope for MVP.
- **Task exceeds timeout (`ZEROCLAW_A2A_TASK_TIMEOUT_SECS`, default 300).** Walter's `AgentExecutor::execute` is wrapped in a `tokio::time::timeout`. On elapsed, Walter calls its own `AgentExecutor::cancel` path, emits `Task{state:canceled}` with reason `"timeout_exceeded"`, and the outbox delivers the statusUpdate to Sam.

## Idempotency

- Every outbox row has unique `(task_id, sequence)`. Receivers check for duplicates on insert → no-op on duplicate. Safe for outbox retries.
- Sam's reasoning loop processes `inbox_events` at-least-once. Turn handler is idempotent on `task_id` completion — completed tasks are terminal; re-processing produces the same result.

## Ordering

- Per task: A2A state transitions are monotonic (submitted → working → completed/failed). Backwards transitions are rejected at the state-machine layer and logged.
- Across tasks: no ordering guarantees. None needed — Sam does not care which of two concurrent delegations completes first.

## Migration plan

Four phases, each independently revertable.

### Phase 1 — Deploy A2A layer alongside ACP (no traffic shift)

- Apply new CNPG cluster `zeroclaw-a2a` with schemas + roles
- Apply VaultStaticSecrets for Sam and Walter DB creds
- Build and deploy new zeroclaw binary with `ra2a` integrated
- Verify: `/.well-known/agent-card.json` resolves on both pods, `tasks/list` returns empty, existing ACP paths unchanged
- Walter still receives real work only via ACP; Sam still delegates only via ACP

### Phase 2 — Cut over one task type

- Pick one delegation: `cluster-health-survey`
- Set `ZEROCLAW_USE_A2A_DELEGATION=true` on Sam's pod
- When Sam sees the feature flag, she routes `cluster-health-survey` through A2A `message/send` + webhook; everything else stays on ACP
- Run end-to-end in prod, observe outbox + webhook timing, confirm loop wakes correctly
- Hold for 1 week. If issues surface, unset the env var — instant revert.

### Phase 3 — Migrate remaining Sam → Walter delegations

- Once Phase 2 is stable, extend the feature flag to cover all delegation paths
- Keep ACP path as fallback code for one release cycle

### Phase 4 (post-MVP) — Deprecate ACP for inter-agent use

- Remove the ACP inter-agent delegation code and the feature flag
- ACP server remains for external callers

## Testing strategy

All testing happens in Kubernetes against a dev namespace.

- **Dev namespace** `zeroclaw-a2a-dev` in the same cluster, running Sam and Walter with a dedicated `zeroclaw-a2a-dev` CNPG cluster. Identical manifests to prod, different namespace + secret scope.
- **Integration tests** run as a K8s Job that hits both agents' A2A endpoints and verifies task lifecycle, retries, and wake behavior end-to-end.
- **Failure injection** via short-lived Cilium NetworkPolicies that drop traffic between Sam and Walter to confirm outbox retries fire.

## Configuration

New env vars introduced:

| Variable | Agent | Purpose | Default |
|---|---|---|---|
| `ZEROCLAW_A2A_DB_URL` | Both | Postgres DSN | (from Vault) |
| `ZEROCLAW_A2A_SCHEMA` | Both | Schema name (`sam` or `walter`) | — |
| `ZEROCLAW_A2A_WEBHOOK_URL` | Sam | Sam's own webhook URL for push configs | — |
| `ZEROCLAW_USE_A2A_DELEGATION` | Sam | Feature flag for Phase 2 cutover | `false` |
| `ZEROCLAW_A2A_TASK_TIMEOUT_SECS` | Walter | Max `execute` duration before forced cancel | `300` |
| `ZEROCLAW_A2A_OUTBOX_MAX_ATTEMPTS` | Both | Retry ceiling before dead-letter | `5` |

## Appendix — decisions log

| Question | Options considered | Decision | Reason |
|---|---|---|---|
| Scope | (A) wake/resume layer (B) externalize loop (C) rewrite | **A** | Smallest wedge; preserves Rust; door open to B/C later |
| MVP triggering use case | (A) inter-agent only (B) +webhooks (C) unify all triggers | **A** | Solves the actual pain; scope widening is straightforward later |
| Wake mechanism | (A) HTTP endpoint (B) durable queue (C) DB + inotify | **B** | Survives receiver offline; matches A2A spec's shape |
| Queue backend | (A) Postgres LISTEN/NOTIFY (B) Redis Streams (C) NATS (D) Kafka | **Postgres (CNPG)** → revised to **A2A webhooks + outbox** after A2A spec review | Matches A2A's actual pattern |
| Directionality | (A) Walter→Sam only (B) bidirectional + ACP (C) bidirectional, ACP deprecated | **A** (in MVP), evolves to C in Phase 4 | Sam→Walter already works via ACP |
| Protocol | Bespoke mailbox vs adopt A2A | **A2A via `ra2a` crate** | Future LangGraph compat; SDK covers spec; save multi-week implementation work |
| Testing | Local vs K8s | **K8s** | Matches deployment target |
| Feature flag mechanism | Env var vs config file | **Env var** | No existing pattern to match |
| Task timeout | — | **5 min default, configurable** | Walter's observations usually <60s; cap prevents runaway |
