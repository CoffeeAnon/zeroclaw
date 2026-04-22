# Sam task-routing collision — remediation plan

**Date:** 2026-04-22
**Status:** proposal for human review (no skill edits applied)
**Scope:** `daily-meeting-summary` v4.2.0, `task-tracking` v1.2.0, `team-coordination` v1.1.0
**Trigger incident:** 2026-04-21 `speakr-daily-summary` cron filed 10 action items (Vikunja #129-138) to project 3 (Sam's inbox) via `vikunja my-task create` instead of to project 1 (Dan's inbox) via `vikunja task batch-create 1`. Several items were actions other people owe Dan, which should never become Vikunja tasks at all.

---

## 1. The collision in one paragraph

`task-tracking` is **`always: true`** and frames its first rule as a universal gate: *"First decision on every inbound Signal message — is this a task? YES → `vikunja my-task create`."* That rule was written for ad-hoc Signal asks (the 2026-04-16 isolation-leak fix) and is correct in that scope. But because it is resident in every system prompt — including isolated cron sessions — it competes with `daily-meeting-summary` step 3, which has its own, more specific routing: extract Dan-bucket action items → dedup → `vikunja task batch-create 1 --file ...` into project 1. During the `speakr-daily-summary` cron, Sam sees both rules and picks the one phrased as a universal decision gate over the one buried inside a step-3 sub-phase, so every action item — including ones others owe Dan — gets treated as "a thing Dan asked me to do" and routed to her own inbox. The per-skill routing is locally correct; what's broken is the absence of precedence when they disagree.

## 2. What the three skills actually say about routing

| Skill | Mode | Routing claim | Project |
|---|---|---|---|
| `daily-meeting-summary` v4.2.0 | `always: false` (description-triggered) | Step 3 phase 3: `vikunja task batch-create 1 --file ...`, filtered to **Dan's bucket** from step 2 | project **1** (Dan's inbox) |
| `task-tracking` v1.2.0 | **`always: true`** (~1070 tokens resident) | First decision on every inbound Signal message → `vikunja my-task create` | project **3** (Sam's inbox, via `VIKUNJA_SAM_PROJECT_ID`) |
| `team-coordination` v1.1.0 | **`always: true`** | Dan asks → code/k8s work → `vikunja team-task create` + ACP dispatch | project **7** (Sam & Walter ops) |

All three are locally well-reasoned. The collision is at the meta level: none of them say *"if another skill's routing is more specific, yield."* `team-coordination` is dragged in only for completeness — it didn't cause this incident, but it lives in the same resident-skill bucket and amplifies the general "every resident skill thinks its routing is primary" problem.

## 3. Secondary bug exposed by the incident

Even before routing is fixed, meeting-summary step 3 phase 1 says *"Start with every concrete action item in **Dan's bucket** from step 2"* — but several of Sam's 10 tasks were items from the *"Action items others owe Dan"* bucket (Dr. Ledger, Rob, Geneep, Thomas Kramer, Andrus). That is a **second failure**: task-tracking's framing ("anything actionable → create task") overwrote the bucket-filter at the candidate-gathering stage. The filter is in the skill, but it gets out-voted. A full fix has to both route to project 1 **and** preserve the Dan-bucket filter.

## 4. Remediation options

### Option A — Carve-out inside `task-tracking`

Add a short opt-out clause: "If you are inside a cron session named `speakr-*` or `*-meeting-*`, or if the active loaded skill is `daily-meeting-summary`, the routing rules in that skill take precedence over this one. Skip the first-decision gate."

- **Pros:** tiny patch, reversible, keeps `task-tracking` resident for its real job (Signal asks). Explicit about *why* the exemption exists.
- **Cons:** couples `task-tracking` to knowledge of specific crons. Adds a second carve-out each time a new skill collides. Doesn't solve the bucket-filter bug on its own.

### Option B — Override inside `daily-meeting-summary` step 3

Prepend step 3 with: "Inside this skill's step 3, ignore `task-tracking`'s `my-task create` rule. Meeting action items go to project 1 via `batch-create`, not Sam's inbox."

- **Pros:** localizes the fix to the skill that needs the behavior. Doesn't require task-tracking to know about cron names.
- **Cons:** asks the model to explicitly countermand an `always: true` skill, which is a skill-vs-skill conflict framed in natural language — brittle if the model doesn't trust one "ignore" sentence against ~1070 resident tokens.

### Option C — Filter at step 2 so only Dan's bucket reaches step 3 (fixes the secondary bug)

Sharpen step 3 phase 1: "Start with every concrete action item that appears under the `## Action items for Dan` heading of today's exec-summary file. Items under `## Action items others owe Dan` are **never** candidates for Vikunja — they exist to help Dan nudge people, and creating tasks for them would put other people's work on Dan's inbox."

- **Pros:** fixes the real harm from 2026-04-21 (other-people tasks in Vikunja). Strengthens the skill regardless of the routing outcome.
- **Cons:** orthogonal to the collision — doesn't stop `my-task create` from firing. Must be combined with A or B.

### Option D — Drop `always: true` from `task-tracking`

Make `task-tracking` description-triggered instead of resident. Rely on Sam's description matcher to load it on inbound Signal turns but not on cron turns.

- **Pros:** structurally cleanest. No cross-cutting rule fighting inside cron sessions. Reduces the resident-skill token budget (~1070 tokens returned).
- **Cons:** description-triggering is probabilistic. The whole reason `task-tracking` was made `always: true` is that Sam was *forgetting* to open tasks on Signal asks (see 2026-04-16 isolation-leak incident). Reverting risks regressing that fix. Higher blast radius than A/B.

### Option E — Explicit skill-precedence frontmatter

Add a `precedence:` or `yields_to:` key to skill frontmatter, parsed by the skill loader, so Sam resolves conflicts deterministically instead of by prose.

- **Pros:** general solution; every future collision gets a structural answer.
- **Cons:** requires a code change in `src/` (skill loader), which this plan's constraints forbid. Also YAGNI-adjacent — we have exactly one collision today. Park for later if the problem recurs.

## 5. Recommended approach: **A + C** (carve-out in `task-tracking` AND bucket-filter sharpening in `daily-meeting-summary`)

**Rationale.** The two bugs are separable and each deserves its own fix:

- The routing bug is best fixed where the cross-cutting rule lives (option A). Adding one carve-out clause to `task-tracking` is smaller and more honest than asking `daily-meeting-summary` to shout-override a resident skill (option B). Carve-outs are explicit and readable; "ignore the other skill" is not.
- The bucket-filter bug is independent of routing and stays broken under every routing option unless step 3 phase 1 is sharpened (option C). This is a minimal-change win even if we later switch strategies.

Together A+C is ~15 lines of YAML diff, fully reversible, touches only skill bodies (no Rust, no loader change, no cluster apply in this plan), and each change carries its own justification in the skill text so future readers understand *why*.

Option D (drop `always: true`) is the right long-term play if A proves insufficient, but it should be its own iteration after we see A+C land cleanly. Option E is deferred — revisit only if a third collision appears.

## 6. Concrete diffs (copy-pasteable)

### 6.1 `k8s/sam/26_task_tracking_skill_configmap.yaml` — add carve-out

Insert a new subsection immediately **after** the "First decision on every inbound Signal message" block (i.e. after the "When in doubt, create the task..." paragraph, before `## Workflow`):

```yaml
    ## When this skill does NOT apply

    This skill is resident because Sam was losing track of ad-hoc
    Signal asks (see 2026-04-16 isolation-leak incident). That's
    its job. It is NOT the right tool when another skill owns the
    routing for a specific workflow:

    - **Inside a cron session** whose name starts with `speakr-` or
      contains `meeting-summary` — `daily-meeting-summary` step 3
      owns Vikunja routing for that run. Action items go to project
      1 (Dan's inbox) via `vikunja task batch-create 1`, not to
      Sam's inbox. Skip the first-decision gate entirely; the skill
      that loaded for the cron already knows what to do.
    - **Inside the `daily-meeting-summary` skill body** at any
      time — its step 3 is the authoritative Vikunja path for
      meeting action items. Do not layer `my-task create` on top.
    - **Inside the `team-coordination` delegation flow** — the
      parent `my-task` for Dan's ask is still yours to open, but
      the code/k8s delegation itself routes via `team-task create`
      in project 7. Don't double-book the same work.

    The general rule: if a more specific skill owns the destination
    project for a piece of work, let it win. Over-tracking in your
    own inbox is no longer cheap when the work belongs somewhere
    else — a task in the wrong project is worse than no task,
    because Dan then has to clean up.
```

Reasoning in-line is deliberate: the point of the skill-writing style in this repo is to explain *why*, not to issue bare MUSTs. A future maintainer reading this carve-out understands the 2026-04-21 incident without having to hunt through git history.

### 6.2 `k8s/sam/21_meeting_summary_skill_configmap.yaml` — sharpen step 3 phase 1

Replace the current phase 1 opening paragraph:

```
    Start with every **concrete** action item in Dan's bucket from step
    2 (today's primary date). Skip vague items ("look into X", "think
    about Y") — Vikunja is for things with a verb, a noun, and ideally
    a deadline.
```

with:

```
    Start with every **concrete** action item that appears under the
    `## Action items for Dan` heading of today's exec-summary file
    (the file you wrote in step 2). Skip vague items ("look into X",
    "think about Y") — Vikunja is for things with a verb, a noun,
    and ideally a deadline.

    Items under `## Action items others owe Dan` are **never**
    candidates here. They exist in the exec summary so Dan can
    nudge people who owe him work; turning them into Vikunja tasks
    on project 1 puts other people's obligations onto Dan's own
    inbox, which is exactly backwards. If Dr. Ledger owes Dan a
    contract draft, that's a line in the exec summary for Dan to
    read, not a task for Dan to complete. The same goes for the
    `## Key decisions and issues` bucket — decisions are wiki
    material (step 5), not task material.

    Concretely: when gathering candidates, read the exec-summary
    file and take **only** the bullet list immediately following
    the `## Action items for Dan` heading, stopping at the next
    `##` heading. Do not merge in items from sibling sections.
```

The "why" is carried in the text — someone later wondering "can I loosen this to also grab others-owe-Dan as FYI tasks?" reads the paragraph and sees the argument against it.

### 6.3 Optional, low-cost: version bumps

- `task-tracking`: `version: 1.2.0` → `1.3.0` (new carve-out section is a behaviour change, not a bugfix).
- `daily-meeting-summary`: `version: 4.2.0` → `4.2.1` (phrasing tightened, behaviour narrowed to previously-documented intent).

No other file needs to change. Nothing in `src/` is touched. No ConfigMap is applied in this plan.

## 7. Test plan — validating on the next `speakr-daily-summary` run

The cron fires 12:00 and 17:00 America/Vancouver weekdays. First verification window after human applies the diff: next weekday midday run.

### 7.1 Loki checks (Sam's pod logs)

Grep the Sam pod logs in the relevant cron window for:

- **Must NOT appear** (the regression signature):
  - `vikunja my-task create` in a turn that also contains `speakr-daily-summary` or the cron prompt literal.
  - Reasoning-trace strings like `"I did make a tool call (vikunja my-task create)"` immediately after reading a `meetings-YYYY-MM-DD.md` file.
- **Should appear:**
  - Exactly one `vikunja task batch-create 1 --file /data/.zeroclaw/workspace/tasks-YYYY-MM-DD.json` call (or zero, if there were no Dan-bucket items that day).
  - If zero candidates survive dedup, the absence of any `batch-create` and no `my-task create` either.

Suggested Loki queries (adjust label selectors to the cluster):

```logql
{app="zeroclaw", pod=~"sam-.*"} |= "speakr-daily-summary" |= "my-task create"
{app="zeroclaw", pod=~"sam-.*"} |= "speakr-daily-summary" |= "batch-create 1"
```

The first should return zero lines for the test run. The second should return at most one line.

### 7.2 Vikunja checks

After the run:

- `vikunja tasks 1 --format json` (project 1, Dan's inbox) — any new `from-meeting` labelled tasks for today should be present; all descriptions should reference a real meeting title. Zero tasks is fine if Dan had no concrete commitments that day.
- `vikunja my-tasks --open` (project 3, Sam's inbox) — should contain **no** new tasks with `from-meeting`-style descriptions or titles that read like meeting action items. New tasks here should only be genuine Signal asks from the day.
- Spot-check: if today's exec summary has any `## Action items others owe Dan` bullets, none of their titles should appear in project 1 tasks.

### 7.3 Reply-to-Dan check

Sam's final Signal reply for the cron run should look like it always did — an exec summary pointer with the count of tasks opened. Whatever reasoning the model emits internally, the external behaviour is unchanged except for the routing target.

### 7.4 Rollback

If the run regresses in any new way (for example Sam skips `batch-create 1` entirely because she's now over-cautious), revert both ConfigMaps to the v1.2.0 / v4.2.0 bodies via `kubectl apply -f` against the previous revision. Both changes are additive-text-only, so `git revert` of the plan application commit is a clean single-step rollback.

## 8. Non-goals

This plan explicitly does **not**:

- Change the skill loader or add a `precedence:` / `yields_to:` frontmatter key (option E). That's a codebase change and out of scope.
- Drop `always: true` from `task-tracking` (option D). We're not reopening the 2026-04-16 isolation-leak fix without evidence that A+C is insufficient.
- Touch `team-coordination` v1.1.0. It didn't cause this incident. The 6.1 carve-out mentions it for completeness but doesn't edit its body.
- Rewrite step 2's exec-summary template. The buckets exist and are correct; only the step-3 filter needed sharpening.
- Retroactively move or delete Vikunja tasks #129-138. That's Dan's call (some may actually be his to do; others should be deleted or moved to a follow-ups note). The plan is about preventing recurrence.
- Backfill a re-run of 2026-04-21's meetings through the corrected skill. Step 3 is explicitly not idempotent across days, and the dedup pass would skip them anyway.
- Change cron scheduling, timezone, or delivery config. Out of scope.

## 9. Handoff summary

1. **What changes:** two ConfigMap YAMLs in `k8s/sam/` (task-tracking adds a carve-out section; meeting-summary sharpens step 3 phase 1).
2. **What does not change:** Rust code, skill loader, cron config, team-coordination skill, meeting-summary script (`speakr-fetch.py`), exec-summary template.
3. **Validation before apply:** human reviews the two diffs here, confirms phrasing matches repo style.
4. **Validation after apply:** next `speakr-daily-summary` cron run (next weekday 12:00 PT), Loki + Vikunja checks per §7.
5. **Remaining risk:** natural-language carve-outs are softer than structural ones. If Sam's next cron run still calls `my-task create` on meeting items, escalate to option D (drop `always: true` from task-tracking) and re-evaluate.
6. **Next recommended action:** apply the diffs, redeploy the two ConfigMaps, wait for the next cron run, run §7 checks, and — per the repo's wiki checkpoint rule — ingest the incident + fix into `wiki/incidents/` once green.
