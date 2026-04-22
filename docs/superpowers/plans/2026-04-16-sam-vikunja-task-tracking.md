# Sam Vikunja Task Tracking Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move Sam's "things Dan asked me to do" tracking out of ephemeral chat memory and into Vikunja on her own inbox board (project #3), so resolved instructions have explicit lifecycle and stop contaminating relevance-scored recall.

**Architecture:** Extend the existing `vikunja` Python CLI with a small set of Sam-scoped commands (`my-tasks`, `my-task open|done|note`) that default to project #3 via a new `VIKUNJA_SAM_PROJECT_ID` env var. Ship a short `task-tracking` skill that tells Sam when to create tasks, when to close them, and how to surface them at session start. No native Rust tool in this phase — `shell:` + CLI keeps context small and avoids a zeroclaw rebuild.

**Tech Stack:** Python 3 stdlib (CLI), K8s ConfigMap + Sandbox subPath mounts, Vikunja REST API, Gemma 4 via LiteLLM.

---

## Background (read before starting)

Sam runs on Gemma 4 26B with a tight context budget — every added tool schema and every reread costs real latency. This plan follows the same pattern as the existing `vikunja-project-manager` skill + `vikunja` CLI bundle: the CLI does the heavy lifting in Python, the skill is a short document that teaches Sam when/how to use it.

Vikunja projects relevant here:
- `#1` Dan's inbox (meeting tasks go here)
- `#3` Sam's inbox — **her task-tracking board** (currently empty)
- `#4` Dan & Sam team board

Sam's Vikunja API token has project-level access but **cannot** resolve users (`/user` and `/users?s=` return 401). Do not pass `--assignee` anywhere in this workflow; the CLI already warns and proceeds without it.

The existing CLI lives in `k8s/sam/20_vikunja_tool_configmap.yaml` (1096 lines). New commands go into that file alongside the existing `cmd_*` functions.

---

## File Structure

- **Modify** `k8s/sam/20_vikunja_tool_configmap.yaml` — add `VIKUNJA_SAM_PROJECT_ID` env var, four new `cmd_my_*` functions, router entries, and help text
- **Create** `k8s/sam/26_task_tracking_skill_configmap.yaml` — new skill ConfigMap with `task-tracking.md`
- **Modify** `k8s/sam/04_zeroclaw_sandbox.yaml` — add skill mount + volume reference
- **Modify** `k8s/sam/03_zeroclaw_configmap.yaml` — add `VIKUNJA_SAM_PROJECT_ID=3` env in the zeroclaw container env block
- **Modify** `scrapyard-wiki/wiki/services/zeroclaw.md` — document the new skill in the Skills list and add a Custom Fork Changes note

---

## Task 1: Verify assumptions and capture project IDs

**Files:**
- No files modified; this is a discovery step whose findings feed Tasks 2–5.

- [ ] **Step 1: Confirm Sam's inbox project ID and Sam's user context**

Run:
```bash
kubectl exec -n ai-agents zeroclaw -c zeroclaw -- vikunja projects
```

Expected output includes a line matching `#3: Sam's inbox` (confirmed 2026-04-16). If this changes, substitute the actual ID everywhere `3` appears in later tasks.

- [ ] **Step 2: Confirm the inbox is empty (no pre-existing contamination)**

Run:
```bash
kubectl exec -n ai-agents zeroclaw -c zeroclaw -- vikunja tasks 3
```

Expected output: `No tasks in project #3.` If tasks exist, review them with Dan before proceeding — they may be test data worth keeping or leftover from an earlier experiment.

- [ ] **Step 3: Confirm the CLI token can create a task in project 3**

Run:
```bash
kubectl exec -n ai-agents zeroclaw -c zeroclaw -- vikunja task create 3 --title 'plan-verification probe — delete me'
```

Expected output: `Task #<N> created: plan-verification probe — delete me`. Record the task ID.

- [ ] **Step 4: Delete the probe**

Run (substitute the ID from Step 3):
```bash
kubectl exec -n ai-agents zeroclaw -c zeroclaw -- vikunja task delete <N>
```

Expected output: `Task #<N> deleted`.

---

## Task 2: Extend the vikunja CLI with Sam-scoped commands

**Files:**
- Modify: `k8s/sam/20_vikunja_tool_configmap.yaml`

The existing CLI exposes `vikunja tasks <project-id>`, `vikunja task create <project-id> ...`, etc. We add a thin Sam-scoped layer that defaults the project to `VIKUNJA_SAM_PROJECT_ID` so Sam never has to remember or pass the number, and provides a one-shot `my-task done` that combines mark-done + optional comment.

### Why env var, not hardcoded

Hardcoding `3` in the CLI couples it to this specific Vikunja deployment. An env var keeps the CLI reusable and makes testing in a dev Vikunja easy.

### Commands added

```
vikunja my-tasks                          # list Sam's tasks (all statuses)
vikunja my-tasks --open                   # only undone
vikunja my-tasks --format json            # programmatic
vikunja my-task <id>                      # show one (thin wrapper around `task show`)
vikunja my-task create <title> [--description ...] [--priority 1-5] [--due YYYY-MM-DD]
vikunja my-task done <id> [--comment "..."]   # mark done + optional comment in one call
vikunja my-task note <id> <comment>       # add a progress comment without closing
```

- [ ] **Step 1: Add the env var declaration near the other globals**

Find the existing block (around line 51-64 of `20_vikunja_tool_configmap.yaml`, inside the Python script under the `vikunja` key):

```python
    BASE_URL = os.environ.get("VIKUNJA_BASE_URL", "http://vikunja.todolist.svc.cluster.local:3456")
    API = f"{BASE_URL}/api/v1"
    TOKEN = os.environ.get("VIKUNJA_API_TOKEN", "")
```

Add immediately after the `TOKEN` line:

```python
    SAM_PROJECT_ID = os.environ.get("VIKUNJA_SAM_PROJECT_ID", "3")
```

- [ ] **Step 2: Add a helper that resolves Sam's project ID once with a clear error**

Add near the other helpers (before the first `def cmd_` function):

```python
    def sam_project_id():
        """Return the configured project ID for Sam's inbox, or exit with a
        descriptive error if it is missing or not numeric."""
        try:
            return int(SAM_PROJECT_ID)
        except (TypeError, ValueError):
            print(
                f"ERROR: VIKUNJA_SAM_PROJECT_ID must be an integer "
                f"(got: {SAM_PROJECT_ID!r}). Set it in the pod env block.",
                file=sys.stderr,
            )
            sys.exit(2)
```

- [ ] **Step 3: Add `cmd_my_tasks`**

Add after the existing `cmd_tasks` function (~line 498):

```python
    def cmd_my_tasks(args):
        """List tasks in Sam's inbox. Thin wrapper over cmd_tasks that
        injects the project ID so Sam never has to remember it.
        """
        # Build a namespace that cmd_tasks can consume directly.
        from types import SimpleNamespace
        ns = SimpleNamespace(
            project_id=sam_project_id(),
            sort=getattr(args, "sort", "position"),
            format=getattr(args, "format", "text"),
            open=getattr(args, "open", False),
        )
        return cmd_tasks(ns)
```

Note: this assumes `cmd_tasks` already understands `--open`. It may not — check by reading its current body. If `--open` filtering is not supported yet, add it to `cmd_tasks` as a filter applied post-fetch: `if getattr(args, "open", False): tasks = [t for t in tasks if not t.get("done")]`. If you add filtering, also register the CLI flag (Step 5) on both `tasks` and `my-tasks`.

- [ ] **Step 4: Add `cmd_my_task` dispatcher with `create`, `show`, `done`, `note` subcommands**

Add after `cmd_my_tasks`:

```python
    def cmd_my_task(args):
        """Dispatcher for `vikunja my-task <sub> ...`. Subs:
          create <title> [--description ...] [--priority N] [--due YYYY-MM-DD]
          show   <id>
          done   <id> [--comment ...]
          note   <id> <comment>
        Delegates to existing cmd_task_create / cmd_task_show /
        cmd_task_update / cmd_task_comment with Sam's project pre-filled.
        """
        from types import SimpleNamespace
        sub = getattr(args, "subcommand", None)
        if sub == "create":
            ns = SimpleNamespace(
                project_id=sam_project_id(),
                title=args.title,
                description=getattr(args, "description", None),
                priority=getattr(args, "priority", None),
                due=getattr(args, "due", None),
                assignee=None,  # token cannot resolve users
            )
            return cmd_task_create(ns)
        if sub == "show":
            ns = SimpleNamespace(task_id=args.task_id)
            return cmd_task_show(ns)
        if sub == "done":
            ns = SimpleNamespace(
                task_id=args.task_id,
                done=True,
                title=None, description=None, due=None,
                priority=None, assignee=None,
            )
            rc = cmd_task_update(ns)
            comment = getattr(args, "comment", None)
            if comment:
                cmd_task_comment(SimpleNamespace(task_id=args.task_id, body=comment))
            return rc
        if sub == "note":
            return cmd_task_comment(SimpleNamespace(task_id=args.task_id, body=args.body))
        print(
            "Usage: vikunja my-task {create|show|done|note} <args>",
            file=sys.stderr,
        )
        sys.exit(2)
```

- [ ] **Step 5: Register the commands and flags in the argparse block**

Find the argparse setup near the bottom of the script (look for `sub = p.add_subparsers`). Add after the existing `task` subcommand registration:

```python
    # my-tasks — list Sam's inbox
    p_mytasks = sub.add_parser("my-tasks", help="List tasks in Sam's Vikunja inbox.")
    p_mytasks.add_argument("--sort", choices=["position", "priority", "due"], default="position")
    p_mytasks.add_argument("--format", choices=["text", "json"], default="text")
    p_mytasks.add_argument("--open", action="store_true", help="Only show undone tasks.")
    p_mytasks.set_defaults(func=cmd_my_tasks)

    # my-task — operate on one of Sam's tasks
    p_mytask = sub.add_parser("my-task", help="Create/show/close/annotate one of Sam's tasks.")
    mt_sub = p_mytask.add_subparsers(dest="subcommand")

    p_mt_create = mt_sub.add_parser("create", help="Create a task in Sam's inbox.")
    p_mt_create.add_argument("title")
    p_mt_create.add_argument("--description", default=None)
    p_mt_create.add_argument("--priority", type=int, choices=range(1, 6), default=None)
    p_mt_create.add_argument("--due", default=None, help="YYYY-MM-DD")

    p_mt_show = mt_sub.add_parser("show", help="Show one of Sam's tasks.")
    p_mt_show.add_argument("task_id", type=int)

    p_mt_done = mt_sub.add_parser("done", help="Mark a task done (optionally with a closing comment).")
    p_mt_done.add_argument("task_id", type=int)
    p_mt_done.add_argument("--comment", default=None)

    p_mt_note = mt_sub.add_parser("note", help="Append a progress comment without closing.")
    p_mt_note.add_argument("task_id", type=int)
    p_mt_note.add_argument("body")

    p_mytask.set_defaults(func=cmd_my_task)
```

- [ ] **Step 6: Add `my-tasks` / `my-task` to the top-level help block**

Find the help block string (search for `vikunja help` or `top-level` in the file). Add lines after the existing `vikunja task ...` entries:

```
vikunja my-tasks [--open] [--sort ...] [--format text|json]
vikunja my-task create "<title>" [--description ...] [--priority 1-5] [--due YYYY-MM-DD]
vikunja my-task show   <id>
vikunja my-task done   <id> [--comment "..."]
vikunja my-task note   <id> "<comment>"
```

- [ ] **Step 7: Validate the YAML is still parseable and the Python is syntactically valid**

Run:
```bash
python3 -c "
import yaml
with open('/home/wsl2user/github_projects/zeroclaw/k8s/sam/20_vikunja_tool_configmap.yaml') as f:
    d = yaml.safe_load(f)
script = d['data']['vikunja']
compile(script, 'vikunja', 'exec')
print('YAML valid, Python syntax OK')
"
```

Expected: `YAML valid, Python syntax OK`.

- [ ] **Step 8: Commit the CLI extension**

Run:
```bash
cd /home/wsl2user/github_projects/zeroclaw
git add k8s/sam/20_vikunja_tool_configmap.yaml
git commit -m "$(cat <<'EOF'
feat(vikunja-cli): add Sam-scoped my-tasks / my-task commands

Thin wrapper over the existing per-project commands that defaults to
VIKUNJA_SAM_PROJECT_ID (#3 in this cluster) so Sam never has to pass
the project id. my-task done combines mark-done + optional comment
in one CLI round-trip to keep tool-call count low.

Refs: task-tracking skill (follow-up commit).
EOF
)"
```

---

## Task 3: Add VIKUNJA_SAM_PROJECT_ID to the pod env

**Files:**
- Modify: `k8s/sam/03_zeroclaw_configmap.yaml` (if env lives in the ConfigMap), or `k8s/sam/04_zeroclaw_sandbox.yaml` (if env is inline on the container)

The CLI already defaults to `"3"` when unset, so this step is defense in depth — makes the intent visible to future operators.

- [ ] **Step 1: Find where VIKUNJA_API_TOKEN is declared**

Run:
```bash
grep -nR 'VIKUNJA_API_TOKEN' /home/wsl2user/github_projects/zeroclaw/k8s/sam/
```

Note the file and line. The new env var goes in the same block.

- [ ] **Step 2: Add VIKUNJA_SAM_PROJECT_ID next to VIKUNJA_API_TOKEN**

Edit the file found in Step 1. After the `VIKUNJA_API_TOKEN` env entry, add:

```yaml
            - name: VIKUNJA_SAM_PROJECT_ID
              value: "3"
```

Match the existing indentation exactly (usually 12 spaces in the sandbox, could be different in a ConfigMap).

- [ ] **Step 3: Validate YAML**

Run:
```bash
python3 -c "import yaml; yaml.safe_load_all(open('<path from Step 1>'))" && echo OK
```

Expected: `OK`.

- [ ] **Step 4: Commit**

```bash
cd /home/wsl2user/github_projects/zeroclaw
git add <path from Step 1>
git commit -m "feat(sam): declare VIKUNJA_SAM_PROJECT_ID=3 in pod env"
```

---

## Task 4: Draft and package the task-tracking skill

**Files:**
- Create: `k8s/sam/26_task_tracking_skill_configmap.yaml`

This skill is deliberately short. Under Compact mode (Sam's current config), Gemma 4 sees only the skill's `name`, `description`, and `location` in the system prompt; it loads the body via `file_read` on demand when triggered. Keeping the description specific and the body practical keeps the loaded-cost low.

- [ ] **Step 1: Create the ConfigMap file with embedded skill content**

Create `k8s/sam/26_task_tracking_skill_configmap.yaml` with content:

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: zeroclaw-skill-task-tracking
  namespace: ai-agents
  labels:
    app: zeroclaw
    component: skill
data:
  task-tracking.md: |
    ---
    name: task-tracking
    version: 1.0.0
    description: >
      Track durable asks from Dan in Vikunja (Sam's inbox, project #3)
      instead of relying on chat memory. Use whenever Dan sends a
      message that contains a discrete deliverable — anything phrased
      as "please X", "can you X", "I need X", "let's do X", or an
      imperative with a concrete outcome. Also use at the start of
      any new session or conversation to check pending work, and
      when Dan asks "what are you working on" / "what's pending" /
      "what's on your plate".
    always: false
    ---

    # Task Tracking

    Your durable task state lives in Vikunja project #3 — "Sam's
    inbox". The `vikunja` CLI provides a Sam-scoped interface:

    ```
    vikunja my-tasks [--open] [--format text|json]
    vikunja my-task create "<title>" [--description "..."] [--priority 1-5] [--due YYYY-MM-DD]
    vikunja my-task show   <id>
    vikunja my-task done   <id> [--comment "what I did"]
    vikunja my-task note   <id> "<progress update>"
    ```

    All of these operate on your inbox automatically — you do not pass
    a project ID.

    ## Why Vikunja and not `memory_store`

    `memory_store` has no lifecycle. A two-week-old "please re-run X"
    sits in SQLite forever and keeps scoring high on relevance, so
    future sessions see it as if it's still pending — even after
    you've already done the work. On 2026-04-16 an isolated cron
    session picked up a three-day-old instruction that way and ran
    the wrong cron three times in a row. Vikunja tasks have
    `done: true|false`, timestamps, and are inspectable from Dan's
    side, so resolved work looks resolved.

    ## When to create a task

    Create a task when Dan's message is a discrete deliverable:

    - Imperative verb with a concrete outcome
      ("please add a footnote to the incident page")
    - A request for something actionable, not conversational
      ("can you figure out the right Vikunja project id?")
    - Anything with a deadline or a "by X" clause
    - Anything whose completion you'll need to report back on

    Do **not** create a task for:

    - Conversation, acknowledgements, emoji-only messages
    - Questions with verbal answers (Dan asks, you reply — no
      deliverable)
    - Status checks ("are you there?", "did X work?")
    - Meta-comments about yourself or tools

    ## Workflow

    **On receiving a task-flavored message:**

    1. `vikunja my-task create "<short title>" --description "<Dan's
       ask verbatim + any context>"` — capture the ID in your reply.
    2. Acknowledge Dan briefly. It's helpful but not required to
       include the task ID: "Got it, tracking as #N."
    3. Do the work.
    4. `vikunja my-task done <id> --comment "<what you did, including
       any IDs / paths / outputs Dan would want to reference>"`.
    5. Reply to Dan with the final result.

    Steps 1 and 4 are the only CLI calls you need per task. Keep the
    description and comment short — Dan will open Vikunja to read
    detail, not your chat replies.

    **On waking / at session start:**

    Before responding to the current message, run
    `vikunja my-tasks --open`. If there are items, consider whether
    the current message relates to one (update it rather than
    creating a duplicate), and whether any other pending item is
    blocked and needs a nudge to Dan.

    **When Dan asks "what are you working on":**

    `vikunja my-tasks --open`, then summarize. One line per task is
    plenty — ID, title, how long it's been open.

    ## Context hygiene

    This skill is Compact-mode: only the description loads by default.
    When you read this body, do not re-read it within the same turn —
    the content is already in your context. Also do not dump the full
    `my-tasks` output into your reply to Dan unless he asked for it;
    summarize.

    ## Progress updates (long-running tasks)

    If a task takes multiple turns or multiple sessions, use
    `vikunja my-task note <id> "<what changed>"` between steps so
    the task history is legible if Dan audits it. Do this sparingly
    — one note per meaningful milestone, not one per tool call.

    ## Failure modes

    - Task creation returns non-zero → the CLI prints the HTTP
      error. Surface it to Dan and proceed without task tracking for
      that ask. Do not retry silently.
    - You're unsure whether a message is a task or conversation →
      err on the side of creating a task. Over-tracking is cheaper
      than under-tracking, and closing a trivial task costs one CLI
      call.
    - Dan cancels an ask mid-flight → `vikunja my-task done <id>
      --comment "cancelled — reason"`. Closing with a cancellation
      note is cleaner than deleting.
```

- [ ] **Step 2: Validate the YAML**

Run:
```bash
python3 -c "
import yaml
d = yaml.safe_load(open('/home/wsl2user/github_projects/zeroclaw/k8s/sam/26_task_tracking_skill_configmap.yaml'))
assert 'task-tracking.md' in d['data']
print('OK', len(d['data']['task-tracking.md']), 'chars')
"
```

Expected: `OK <N> chars` where N is in the 3000-4000 range.

- [ ] **Step 3: Commit**

```bash
cd /home/wsl2user/github_projects/zeroclaw
git add k8s/sam/26_task_tracking_skill_configmap.yaml
git commit -m "$(cat <<'EOF'
feat(k8s/sam): add task-tracking skill (v1.0.0)

New skill that routes durable asks from Dan into Vikunja project #3
(Sam's inbox) via the new vikunja my-tasks/my-task commands. Under
Compact skill injection mode only the description is resident in
every system prompt; the body loads on demand when Sam's trigger
heuristics fire.

Motivated by the 2026-04-16 isolated cron cascade where a resolved
three-day-old chat instruction surfaced as a phantom pending task.
Moving asks out of memory and into Vikunja gives them an explicit
lifecycle (done: true|false) and an inspectable surface for Dan.
EOF
)"
```

---

## Task 5: Mount the skill into the Sandbox pod

**Files:**
- Modify: `k8s/sam/04_zeroclaw_sandbox.yaml`

- [ ] **Step 1: Find the existing `skill-*` mount block and add a new mount for task-tracking**

In `04_zeroclaw_sandbox.yaml`, find a block like:

```yaml
            - name: skill-wiki-management
              mountPath: /data/.zeroclaw/workspace/skills/wiki-management/SKILL.md
              subPath: wiki-management.md
              readOnly: true
```

Add an analogous block for task-tracking, preserving indentation:

```yaml
            - name: skill-task-tracking
              mountPath: /data/.zeroclaw/workspace/skills/task-tracking/SKILL.md
              subPath: task-tracking.md
              readOnly: true
```

- [ ] **Step 2: Add the volume reference near the other `skill-*` volumes**

In the same file, find a block like:

```yaml
        - name: skill-wiki-management
          configMap:
            name: zeroclaw-skill-wiki-management
```

Add an analogous block:

```yaml
        - name: skill-task-tracking
          configMap:
            name: zeroclaw-skill-task-tracking
```

- [ ] **Step 3: Add a `mkdir -p` for the new skill directory in the init container**

In the same file, find the init command that does `mkdir -p /data/.zeroclaw/workspace/skills/wiki-management` and similar. Add a line:

```bash
              mkdir -p /data/.zeroclaw/workspace/skills/task-tracking
```

Match the surrounding indentation exactly.

- [ ] **Step 4: Validate YAML**

Run:
```bash
python3 -c "import yaml; list(yaml.safe_load_all(open('/home/wsl2user/github_projects/zeroclaw/k8s/sam/04_zeroclaw_sandbox.yaml')))" && echo OK
```

Expected: `OK`.

- [ ] **Step 5: Commit the Sandbox change alone (so the rollout is reviewable)**

```bash
cd /home/wsl2user/github_projects/zeroclaw
git add k8s/sam/04_zeroclaw_sandbox.yaml
git commit -m "feat(k8s/sam): mount task-tracking skill ConfigMap"
```

---

## Task 6: Apply and verify

**Files:** No files modified in this task.

- [ ] **Step 1: Apply the new ConfigMap first**

Run:
```bash
kubectl apply -f /home/wsl2user/github_projects/zeroclaw/k8s/sam/26_task_tracking_skill_configmap.yaml
```

Expected: `configmap/zeroclaw-skill-task-tracking created`.

- [ ] **Step 2: Apply the updated vikunja CLI ConfigMap and env**

Run:
```bash
kubectl apply -f /home/wsl2user/github_projects/zeroclaw/k8s/sam/20_vikunja_tool_configmap.yaml
kubectl apply -f <path from Task 3 Step 1>
```

Expected: both `configmap/... configured` or `unchanged`.

- [ ] **Step 3: Apply the Sandbox change**

Run:
```bash
kubectl apply -f /home/wsl2user/github_projects/zeroclaw/k8s/sam/04_zeroclaw_sandbox.yaml
```

Expected: `sandbox.agents.x-k8s.io/zeroclaw configured`.

- [ ] **Step 4: Roll the pod to pick up the new subPath mounts and ConfigMap changes**

Run:
```bash
kubectl delete pod zeroclaw -n ai-agents
```

Watch for the new pod:

```bash
kubectl get pods -n ai-agents -w
```

Wait until `zeroclaw` shows `2/2 Running`. Press Ctrl-C.

- [ ] **Step 5: Verify the skill file is mounted**

Run:
```bash
kubectl exec -n ai-agents zeroclaw -c zeroclaw -- cat /data/.zeroclaw/workspace/skills/task-tracking/SKILL.md | head -20
```

Expected: the frontmatter (`---`, `name: task-tracking`, `version: 1.0.0`, etc.).

- [ ] **Step 6: Verify the CLI has the new commands**

Run:
```bash
kubectl exec -n ai-agents zeroclaw -c zeroclaw -- vikunja my-tasks
```

Expected: either an empty-project message or a task list — **not** `vikunja: command not found` and **not** an argparse error about `my-tasks` being unrecognized.

- [ ] **Step 7: Verify the env var is set**

Run:
```bash
kubectl exec -n ai-agents zeroclaw -c zeroclaw -- env | grep VIKUNJA_SAM
```

Expected: `VIKUNJA_SAM_PROJECT_ID=3`.

---

## Task 7: End-to-end smoke test

**Files:** No files modified in this task.

The plan produces working behavior, but "Sam actually follows the skill" is only testable against the real agent. Do this by sending her a simple task-flavored message and verifying she lands it in Vikunja correctly.

- [ ] **Step 1: Confirm Sam's inbox is empty before the test**

Run:
```bash
kubectl exec -n ai-agents zeroclaw -c zeroclaw -- vikunja my-tasks
```

Expected: `No tasks in project #3.` If anything is there, note the IDs so you can distinguish test output.

- [ ] **Step 2: Send Sam a task-flavored Signal message (manual step for Dan)**

Suggested test prompt, copy-pasteable to Dan:

> Send Sam a Signal message like: "Can you check what version of Python is running inside your pod and reply with the result? No rush."

This is deliberately trivial so the full Create → Do → Done cycle runs in one session without other tool loops muddying the test.

- [ ] **Step 3: Verify a task was created in Sam's inbox**

Within ~2 minutes of Dan sending the message, run:

```bash
kubectl exec -n ai-agents zeroclaw -c zeroclaw -- vikunja my-tasks
```

Expected: exactly one task with a title describing the ask (something like "Check Python version" or "Report Python version in pod"). Record the task ID.

- [ ] **Step 4: Verify the task is closed after Sam replies**

After Sam sends her reply to Dan, run:

```bash
kubectl exec -n ai-agents zeroclaw -c zeroclaw -- vikunja my-task show <id>
```

Expected: task shows `done: true`, with a closing comment describing what she did.

- [ ] **Step 5: Cleanup the smoke-test task (optional — Dan may want to keep it)**

If Dan wants to keep the test task as proof: leave it.
If not:

```bash
kubectl exec -n ai-agents zeroclaw -c zeroclaw -- vikunja task delete <id>
```

- [ ] **Step 6: If Sam did NOT create a task, iterate on the skill description**

The most likely failure mode is under-triggering — Sam decides the message is "conversational" and skips task creation. If so:

1. Check Sam's reasoning in `kubectl logs -n ai-agents zeroclaw -c zeroclaw --tail=200`. Look for her decision about whether to use task-tracking.
2. If she never loaded the skill at all, the `description` field isn't specific enough to trigger. Tighten it — add concrete example phrases Dan actually uses.
3. Re-apply the ConfigMap and roll the pod.

Do **not** change the skill body to be more forceful ("ALWAYS CREATE A TASK"). That fights Compact mode's value proposition. Tune the description instead.

---

## Task 8: Document in the scrapyard wiki

**Files:**
- Modify: `~/github_projects/scrapyard-wiki/wiki/services/zeroclaw.md`

- [ ] **Step 1: Add task-tracking to the Skills list**

Find the `### Skills` heading (near line 62). Add `- task-tracking` to the bullet list:

```markdown
### Skills

- wiki-management
- daily-meeting-summary
- cron-management
- k8s-delegation
- vikunja-project-manager
- browser-navigation
- science-curator (v3.0.0 — see below)
- task-tracking (v1.0.0 — see below)
```

- [ ] **Step 2: Add a subsection describing the skill**

Under the Skills list, after `#### science-curator v3.0.0 (rewritten 2026-04-13)`, add:

```markdown
#### task-tracking v1.0.0 (added 2026-04-16)

Routes durable asks from Dan into Vikunja project #3 (Sam's inbox)
via new `vikunja my-tasks` / `vikunja my-task` commands. Under
Compact skill injection mode only the short description is resident
in every system prompt; the body loads on demand when Sam judges
the inbound message to be a discrete deliverable vs a
conversational turn.

Motivated by [[incidents/2026-04-16-sam-isolated-cron-session-memory-leak]]
— a three-day-old chat instruction kept surfacing as a phantom
pending task because `memory_store` has no lifecycle. Vikunja tasks
do (`done: true|false`, timestamps, Dan-inspectable), so resolved
work looks resolved.

CLI surface added to `k8s/sam/20_vikunja_tool_configmap.yaml`:
- `vikunja my-tasks [--open]` — list Sam's tasks
- `vikunja my-task create "<title>" [flags]` — create in her inbox
- `vikunja my-task done <id> [--comment "..."]` — close + optional note
- `vikunja my-task note <id> "<body>"` — progress update without closing

Project ID is read from `VIKUNJA_SAM_PROJECT_ID` (default `3`).

No retroactive import of pre-existing chat-memory instructions;
the skill starts fresh on first use. Old relevance-scored entries
will decay naturally (memory_loader half-life is 7 days).
```

- [ ] **Step 3: Commit**

```bash
cd /home/wsl2user/github_projects/scrapyard-wiki
git add wiki/services/zeroclaw.md
git commit -m "$(cat <<'EOF'
wiki: document task-tracking skill (v1.0.0)
EOF
)"
```

---

## Self-review checklist for the engineer

Before opening a PR / calling this done:

- [ ] All 8 tasks marked complete
- [ ] Sam's inbox has at least one round-tripped task (Task 7 Step 4 passed)
- [ ] No hardcoded `3` in the CLI — only `SAM_PROJECT_ID` / env reads
- [ ] `vikunja my-tasks --help` prints the new commands
- [ ] The skill's `description` field is specific enough that Gemma 4 actually loads it (re-check logs of Task 7 Step 2 — look for `file_read ... task-tracking/SKILL.md`)
- [ ] No mentions of the old chat-memory-as-task pattern in the committed skill body
- [ ] Wiki entry links to the isolation-leak incident page

## Non-goals (explicitly out of scope)

- A native Rust tool for task operations (deferred until we see Compact-mode performance data)
- Retroactive migration of old chat-memory instructions into Vikunja
- Signal channel-side auto-task-creation on message receipt
- Multi-assignee task routing (the API token can't resolve users anyway)
- Cross-project task tracking beyond Sam's inbox
