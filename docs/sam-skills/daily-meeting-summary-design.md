# `daily-meeting-summary` skill — design notes

Rationale, history, edge-case troubleshooting, and API reference for the
`daily-meeting-summary` skill. These live here rather than in the
`SKILL.md` body so Sam's runtime context stays small — the skill is
loaded into every `speakr-daily-summary` cron turn, and every paragraph
resident at runtime pushes the total prompt closer to the provider HTTP
timeout on long inference.

Scope: this doc is for humans maintaining the skill. Sam does not read it
at runtime.

Canonical skill body: `k8s/sam/21_meeting_summary_skill_configmap.yaml`.

## Why absolute paths are mandatory

The wiki and meeting files live at `/data/workspace/wiki/` and
`/data/workspace/memory/`. Sam's cwd is `/data/.zeroclaw/workspace/` —
a different directory on the same PVC. Relative paths like
`meetings-2026-04-13.md` silently resolve into cwd and miss the real
files. `workspace_only = false` in the security policy is what makes
absolute paths reachable; relative paths still join against cwd and land
in the wrong tree.

The short version in the skill body ("always use absolute paths for
meeting/wiki content") is all the runtime needs. This longer explanation
is here for anyone debugging "file not found" on a file that obviously
exists.

## Why "don't re-read in the same turn" is a load-bearing rule

If Sam reads a file with `file_read` or `shell: cat` and then reads it
again in the same turn, every subsequent request body carries the file's
content twice. For a 10 KB meeting file that's ~2500 tokens per re-read.
Three re-reads and the system prompt plus history can exceed ~40k
tokens, which on `litellm-sam-cron` (Tier 0.5) and Qwen 3.6 35B-A3B
reliably exceeds the 300s HTTP timeout in `reliability.provider_request_timeout_secs`.
The observed failure mode is a cascade of `error sending request for url`
retries followed by eventual cron failure.

Root incident: 2026-04-13 Friday-meeting-summary cascade. Fix: v1.5.13
raised the provider timeout from 120s to 300s, but the root driver was
Sam's habit of re-reading the same meeting file during Step 3 candidate
gathering. The runtime rule is "don't re-read"; this doc records why.

If Sam genuinely needs a slice of an already-read file (e.g., to verify
a specific section wasn't truncated), use `file_read` with `offset` and
`limit` to pull only the relevant lines rather than re-loading the full
file.

## Cron prompt (canonical reference)

The `speakr-daily-summary` cron's prompt in `cron_jobs.db` is the
source of truth. The skill body used to duplicate it; it no longer
does. Reference copy kept here so maintainers can check drift:

```
You are in an isolated cron session for speakr-daily-summary.
Run the daily-meeting-summary skill: steps 1 through 5, in order.
Do not call cron_run — you are already inside the cron job.
If step 1's output contains a `NO_CHANGES:` line, your final reply
must be literally `NO_REPLY` and nothing else — do not run steps
2 through 5. Otherwise continue through step 5.
```

If either the DB row or this file drifts from the other, the DB row
wins (Sam reads the DB, not this file).

### Cron bootstrap on a fresh Sam deployment

On a brand-new Sam deployment the cron needs to be created once:

- name: `speakr-daily-summary`
- schedule: `0 12,17 * * 1-5`
- timezone: `America/Vancouver`
- job_type: `agent`
- session_target: `isolated`
- prompt: the block above, verbatim
- delivery: `{"mode":"none","best_effort":true}` (Sam's in-workflow
  `send_user_message` handles delivery; announce-mode with
  `channel:telegram` was the 2026-04-16 misconfig that produced
  `delivery.channel is required` warnings).

Done via `cron_add` once; persisted in `/data/.zeroclaw/workspace/cron/jobs.db`.

## Step 1 — fetch-script output grammar (reference)

`speakr-fetch.py` emits one of:

| Output | Meaning | Sam must |
|---|---|---|
| `NO_CHANGES: ...` | Zero new meetings today, nothing pending, no garbage, no late transcripts, no annotations | Reply literally `NO_REPLY`, stop. |
| `Wrote N meetings to meetings-YYYY-MM-DD.md. [K failed.] [M pending.] [G garbage.] [New since last run: ...]` | Primary-date summary | Continue to step 2. Surface garbage count to Dan if `G > 0`. |
| `CHANGED sources (late transcripts): files` | Prior days whose transcripts landed since last run — action items NOT captured by previous cron | Run step 3 on each. Skip step 2. Prefix task descriptions `(late backfill from YYYY-MM-DD)`. |
| `CHANGED sources (annotated): files` | Prior days that already had meeting content; Dan edited them | Re-distill in step 5 only. DO NOT re-run step 3. |

Single period-separated line for the primary-date result keeps stdout
parsable by both humans and the skill. Each field only appears when it
applies.

## Step 3 design decisions

### Why project 1 (Dan's inbox), not project 3 (Sam's)

Meeting action items are deliverables for Dan. The `task-tracking`
skill's `vikunja my-task create` goes to project 3 (Sam's inbox via
`VIKUNJA_SAM_PROJECT_ID`). That's wrong for meeting items. Step 3 uses
`vikunja task batch-create 1` explicitly.

Background: 2026-04-21 incident. `task-tracking` (always: true) overrode
meeting-summary's routing and Sam filed 10 meeting items to project 3.
Fixed 2026-04-22 with a carve-out in `task-tracking` ("skip this gate
inside `speakr-*` crons") and this explicit-heading rule in step 3 phase
1. See `2026-04-22-sam-task-routing-collision.md` in `docs/superpowers/plans/`.

### Why "others owe Dan" items are never candidates

Creating a Vikunja task for "Dr. Ledger to send FLUTD links" puts Dr.
Ledger's obligation on Dan's inbox. The exec-summary "Action items
others owe Dan" section exists so Dan can nudge people; turning them
into tasks inverts the relationship. Same principle for "Key decisions
and issues" — decisions are wiki material (step 5), not task material.

### Why one batch-create call, not N task creates

Every separate `shell:` call crosses the provider boundary with its
own HTTP timeout window. A typical review produces ~5 tasks. Five
create calls plus five label calls is 10 timeout windows; one
`batch-create` is one. Under Tier-0.5 cron load this matters — Sam has
historically timed out mid-batch on multi-call stacks.

### Why `--file` not `--stdin` for the JSON

Passing multi-item JSON through `--stdin` heredocs in shell works in a
pinch, but shell-escaping is brittle when descriptions contain quotes
or apostrophes. The tmp file path avoids the escape layer entirely —
the CLI reads the file bytes, parses JSON, and never re-parses through
shell.

### Why leave `tasks-YYYY-MM-DD.json` in place

It's a compact, human-readable record of exactly what Sam tried to
create on that day. The fastest answer when Dan asks "what did you put
in Vikunja yesterday?" is `cat /data/.zeroclaw/workspace/tasks-YYYY-MM-DD.json`.
Periodic cleanup is out of scope for this skill.

### Why no `--assignee dan`

This Sam instance's Vikunja API token has no user-scope access. Both
`GET /users?s=dan` and `GET /user` return 401. The CLI catches the
error and exits 0 (task was created, just not assigned), but the
warning is noise. Tasks land in Dan's inbox correctly via project
ownership — assignment is redundant.

Landed as the `d317ec3a` fix on 2026-04-14 in the vikunja CLI. Prior
behavior: single-create path exited 1 even on success, cron sessions
saw non-zero exits after every meeting task.

### Priority selection rule

- `priority: 4` if the meeting gave a deadline within the next 7 days
- `priority: 3` otherwise

Rationale: meeting action items have deadlines and provenance, so they
shouldn't land at priority 0 next to generic backlog items. Priority 4
gets Dan's eye on same-week commitments; priority 3 is "real, just not
urgent."

## Speakr API reference

- Base URL: `https://meetings.coffee-anon.com`
- Auth: `Bearer $SPEAKR_API_TOKEN`
- Query syntax: `q=date:today`, `q=date:yesterday`, `q=date:YYYY-MM-DD`,
  `q=date:thisweek`, `q=date_from:X&date_to:Y`
- Pagination: add `per_page=100` to avoid pagination
- Primary endpoints:
  - `GET /api/recordings?q=...` — list
  - `GET /api/recordings/{id}` — full detail with transcription

## Extended failure modes

Covered in the skill body (brief): `NO_CHANGES`, `file_read` not-found,
mid-turn provider timeouts. The additional cases below stay here
because they're rare:

- **`SPEAKR_API_TOKEN` missing** → `speakr-fetch.py` raises `KeyError`
  before any API call. Means `shell_env_passthrough` dropped the token
  or the secret isn't mounted. Check the Sandbox spec's `env:` block
  and the `zeroclaw-config-secrets` Secret. Surface to Dan and stop.
- **Speakr HTTP 5xx or connection error** → retry the script once. If
  it fails again, surface to Dan and stop. Meeting transcripts aren't
  tolerant of hour-long recovery windows; Dan prefers to know it's
  broken rather than miss a day of captured action items.
- **Vikunja CLI error on a specific task** → log the title, continue
  with the rest of step 3. Do not block step 4 or step 5. Vikunja is
  a nice-to-have output, not the workflow's core contract.
- **`vikunja ... --assignee dan` warning on stderr but exit 0** → the
  task was created without an assignee. You're running an older call
  path that passes `--assignee`. Drop the flag; tasks still land
  correctly via project ownership.
- **Primary file contains `Warning: summary appears malformed` banners**
  → skip those specific recordings in step 3 (no Vikunja task) and
  step 5 (no wiki page). Surface the count + recording IDs to Dan so
  he can trigger a manual re-summarize in Speakr's UI.

## History

- **v4.3.0 (2026-04-22)** — split into lean SKILL.md + this design
  doc. Removed 10-line absolute-paths explanation, 22-line
  context-hygiene paragraph, duplicate cron prompt, deep Step 3
  rationale, API reference, and extended failure modes. Body went
  from ~498 lines to ~200. Behavior unchanged; the goal was to
  stop Qwen 3.6 35B attention falloff on long tool_result content.
- **v4.2.1 (2026-04-22)** — sharpened Step 3 Phase 1 to take
  **only** items under `## Action items for Dan` heading; explicitly
  excluded "Action items others owe Dan" and "Key decisions and
  issues" from ever becoming Vikunja tasks. Paired with `task-tracking`
  v1.3.0 carve-out.
- **v4.2.0 (2026-04-13)** — wiki distillation delegated to the
  `wiki-management` skill instead of duplicating the page-format
  rules.
- **v4.1.0 → v4.2.0 (2026-04-13)** — absolute paths enforced
  everywhere after Sam's cwd `/data/.zeroclaw/workspace/` silently
  misresolved relative paths.
- **v4.0 and earlier** — pre-Gemma-4-26B-A4B era. Don't re-read.
