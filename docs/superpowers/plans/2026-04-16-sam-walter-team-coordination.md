# Sam–Walter Team Coordination Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give Walter a Vikunja-based coordination surface with Sam so that Dan→Sam→Walter delegations are durable, inspectable, and async-safe. Walter starts every ACP session by checking the shared board; Sam creates tasks there before dispatching via `acp-client`.

**Architecture:** New shared Vikunja project (`#5 Sam & Walter ops`), a new single skill `team-coordination` with `always: true` (applying the v1.2.0 lesson from Sam's task-tracking), and extended `vikunja` CLI with `team-tasks` / `team-task` subcommands. Walter gets the Vikunja CLI and a new API token (Vault-backed). Both agents mount the same skill ConfigMap so the rules stay in sync.

**Tech Stack:** Python 3 stdlib (CLI extension), K8s ConfigMap + Sandbox mounts, Vikunja REST API, HashiCorp Vault via VSO, ACP-over-HTTP (`acp-client`).

---

## Background (read before starting)

- **Walter** (`zeroclaw-k8s-agent`): ACP server for Gitea PR review and k8s operations. CLI-only channel (no Signal). Currently on `citizendaniel/zeroclaw-sam:v1.5.0` — **15 versions behind Sam**. Manifest references `v1.4.12` (drifted).
- **Sam** (`zeroclaw`): Signal-facing agent, runs `v1.5.15`, has the `task-tracking` skill (v1.2.0, `always: true`) for Dan-facing work. Uses `acp-client` to delegate to Walter.
- **Vikunja projects today:** `#1 Dan's inbox`, `#3 Sam's inbox` (task-tracking), `#4 Dan & Sam team board`. This plan adds `#5 Sam & Walter ops`.
- **Lesson carried forward (v1.2.0 tuning on task-tracking):** Compact-mode description-triggered skill loading doesn't work for behavior-override skills. This plan uses `always: true` from the start.

### Walter's skill-mount pattern is different from Sam's

- **Sam** mounts one ConfigMap per skill via `subPath`, each at `/data/.zeroclaw/workspace/skills/<name>/SKILL.md`.
- **Walter** has one combined ConfigMap (`zeroclaw-k8s-agent-skills`) mounted at `/etc/zeroclaw-template/skills/`. His init container iterates `*.md` there and copies each to `/data/.zeroclaw/workspace/skills/<stem>/SKILL.md`.

To keep a single source of truth for `team-coordination.md`, we create ONE standalone ConfigMap that both pods mount:

- Sam mounts it directly at `/data/.zeroclaw/workspace/skills/team-coordination/SKILL.md` via `subPath` (matches her pattern).
- Walter mounts it at `/etc/zeroclaw-template/skills-extra/team-coordination.md` and extends his init script to also iterate that directory.

---

## Prerequisite: Dan-only setup (Task 1)

Three things must exist before any code change takes effect:

1. **Vikunja user `walter`** with a long-lived API token. Use the same token-scope conventions as the existing `sam` user. The token should not need user-operation scope (Walter will work with tasks, not users).
2. **Vikunja project `#5 Sam & Walter ops`** (exact ID may differ — confirm before committing manifests). Both `sam` and `walter` users have admin access so both can create/close/comment.
3. **Vault secret:** add a new key `vikunja-api-token` to the KV-v2 path that backs Walter's VSO destination `zeroclaw-k8s-agent-secrets` (mount `kvv2`, path `zeroclaw-k8s-agent/zeroclaw-k8s-agent-secret` — verify with `kubectl get vaultstaticsecret -n ai-agents zeroclaw-k8s-agent-secret -o yaml`). Value = the Walter user's Vikunja API token from step 1.

These are atomic and reversible. If the project ID isn't 5, Tasks 4–6 need to reference the correct ID instead.

---

## File Structure

**Create:**
- `k8s/shared/27_team_coordination_skill_configmap.yaml` — shared skill ConfigMap mounted on both pods (single source of truth for `team-coordination.md`)
- `docs/superpowers/plans/2026-04-16-sam-walter-team-coordination.md` — this plan document (already exists once this is being executed)

**Modify:**
- `k8s/sam/20_vikunja_tool_configmap.yaml` — add `team-tasks` and `team-task` subcommands paralleling the existing `my-tasks` / `my-task`, driven by `VIKUNJA_TEAM_PROJECT_ID`.
- `k8s/sam/03_zeroclaw_configmap.yaml` — add `VIKUNJA_TEAM_PROJECT_ID` to `shell_env_passthrough`.
- `k8s/sam/04_zeroclaw_sandbox.yaml` — add `VIKUNJA_TEAM_PROJECT_ID=5` env + mount new team-coordination skill.
- `k8s/walter/01_configmap.yaml` — add Vikunja vars to Walter's `shell_env_passthrough`.
- `k8s/walter/03_sandbox.yaml` — bump image `v1.5.0` → `v1.5.15` (current running is drifted up from v1.4.12), add `VIKUNJA_API_TOKEN` / `VIKUNJA_BASE_URL` / `VIKUNJA_TEAM_PROJECT_ID` env from Vault-backed secret, mount the `zeroclaw-vikunja-tool` ConfigMap as `/usr/local/bin/vikunja`, mount the new team-coordination ConfigMap at `/etc/zeroclaw-template/skills-extra`, extend the init script to iterate that directory too.

Note: the `zeroclaw-vikunja-tool` ConfigMap (CLI) already exists in the `ai-agents` namespace; the file path `k8s/sam/20_...` is convention, not scope — Walter can reference it by ConfigMap name.

---

## Task 1: Prerequisite setup (Dan)

**Actor:** Dan (manual steps outside the code change).

This task is NOT blocked by and does not block the code tasks, but the deploys in Task 6 will fail health checks if Vault doesn't have the new key by then.

- [ ] **Step 1: Confirm project ID**

  Verify the next-available Vikunja project ID is 5 (or record the actual new ID for substitution later):

  ```
  kubectl exec -n ai-agents zeroclaw -c zeroclaw -- vikunja projects
  ```

  Expected: a line `#5: <...>` after creation, matching "Sam & Walter ops" or equivalent.

- [ ] **Step 2: Vikunja user and project**

  Create a Vikunja user `walter` (any email; agents don't read email). Create project "Sam & Walter ops" with both users as admins. Generate a long-lived API token for the `walter` user.

- [ ] **Step 3: Vault**

  Add a `vikunja-api-token` key to the KV-v2 secret at the path that VSO reads for Walter. Verify with:

  ```bash
  kubectl get vaultstaticsecret -n ai-agents zeroclaw-k8s-agent-secret -o jsonpath='{.spec.mount}/{.spec.path}'; echo
  ```

  Expected: something like `kvv2/zeroclaw-k8s-agent/zeroclaw-k8s-agent-secret`. Add the key under that path.

- [ ] **Step 4: Confirm VSO sync**

  Force-refresh VSO (or wait 30s) and check the K8s secret:

  ```bash
  kubectl get secret -n ai-agents zeroclaw-k8s-agent-secrets -o jsonpath='{.data.vikunja-api-token}' | base64 -d | head -c 20; echo
  ```

  Expected: the first few characters of the token you generated.

- [ ] **Step 5: Report project ID**

  If the project ID is not 5, report the actual ID to the implementer — Tasks 4 and 5 need it. Otherwise, proceed.

---

## Task 2: Extend the `vikunja` CLI with team-* commands

**Files:**
- Modify: `k8s/sam/20_vikunja_tool_configmap.yaml`

The existing CLI has `my-*` commands that default to `VIKUNJA_SAM_PROJECT_ID`. Add a parallel pair of subcommands that default to `VIKUNJA_TEAM_PROJECT_ID` so both agents can use the same CLI surface for the shared board. Same patterns apply (use raw `list[str]` arg construction, not `SimpleNamespace` — this was the deviation discovered during the `my-*` implementation).

### Commands added

```
vikunja team-tasks [--open] [--sort position|priority|due] [--format text|json]
vikunja team-task create "<title>" [--description "..."] [--priority 1-5] [--due YYYY-MM-DD]
vikunja team-task show   <id>
vikunja team-task done   <id> [--comment "..."]
vikunja team-task note   <id> "<body>"
```

All default to `VIKUNJA_TEAM_PROJECT_ID` (integer, required — exits 2 if unset/non-numeric, parallel to `sam_project_id()`).

- [ ] **Step 1: Add the env var declaration**

  Near line 72 of the Python script, after `SAM_PROJECT_ID`:

  ```python
      TEAM_PROJECT_ID = os.environ.get("VIKUNJA_TEAM_PROJECT_ID", "")
  ```

  No default — this skill is opt-in per pod. If a pod doesn't set it and calls `team-*`, they get a clean error.

- [ ] **Step 2: Add the helper**

  After `sam_project_id()` (around line 393):

  ```python
      def team_project_id():
          """Return the configured project ID for the Sam-Walter shared board,
          or exit with a descriptive error if it is missing or not numeric."""
          if not TEAM_PROJECT_ID:
              print(
                  "ERROR: VIKUNJA_TEAM_PROJECT_ID is not set. The team-* "
                  "commands require a shared project ID; set it in the pod "
                  "env block (e.g. 5 for 'Sam & Walter ops').",
                  file=sys.stderr,
              )
              sys.exit(2)
          try:
              return int(TEAM_PROJECT_ID)
          except (TypeError, ValueError):
              print(
                  f"ERROR: VIKUNJA_TEAM_PROJECT_ID must be an integer "
                  f"(got: {TEAM_PROJECT_ID!r}).",
                  file=sys.stderr,
              )
              sys.exit(2)
  ```

- [ ] **Step 3: Add `cmd_team_tasks`**

  After `cmd_my_tasks`. This is a thin wrapper that builds a raw arg list and delegates to `cmd_tasks`:

  ```python
      def cmd_team_tasks(args):
          """List tasks in the Sam-Walter shared project. Thin wrapper over
          cmd_tasks that injects the team project ID."""
          flags, _ = _parse_args(args, {
              "sort": str, "format": str, "open": bool,
          })
          delegate_args = [str(team_project_id())]
          delegate_args += ["--sort", flags.get("sort", "position")]
          delegate_args += ["--format", flags.get("format", "text")]
          if flags.get("open"):
              delegate_args.append("--open")
          return cmd_tasks(delegate_args)
  ```

- [ ] **Step 4: Add `cmd_team_task` dispatcher**

  After `cmd_team_tasks`, mirroring `cmd_my_task` structure. **IMPORTANT:** use raw `list[str]` args when calling delegate functions — the existing CLI does not use `SimpleNamespace` (this was the deviation discovered during the my-* implementation).

  ```python
      def cmd_team_task(args):
          """Dispatcher for `vikunja team-task <sub> ...`. Delegates to existing
          cmd_task_create / show / update / comment with the team project pre-filled.
          Same shape as cmd_my_task."""
          if not args:
              print("Usage: vikunja team-task {create|show|done|note} <args>",
                    file=sys.stderr)
              sys.exit(2)
          sub, rest = args[0], args[1:]

          if sub == "create":
              flags, positional = _parse_args(rest, {
                  "description": str, "priority": int, "due": str,
              })
              if not positional:
                  print("Usage: vikunja team-task create <title> [flags]",
                        file=sys.stderr)
                  sys.exit(2)
              title = positional[0]
              delegate_args = [str(team_project_id()), "--title", title]
              for k, v in flags.items():
                  delegate_args += [f"--{k}", str(v)]
              return cmd_task_create(delegate_args)

          if sub == "show":
              _, positional = _parse_args(rest, {})
              if not positional:
                  print("Usage: vikunja team-task show <id>", file=sys.stderr)
                  sys.exit(2)
              return cmd_task_show([positional[0]])

          if sub == "done":
              flags, positional = _parse_args(rest, {"comment": str})
              if not positional:
                  print("Usage: vikunja team-task done <id> [--comment \"...\"]",
                        file=sys.stderr)
                  sys.exit(2)
              task_id = positional[0]
              # cmd_task_update calls sys.exit(1) on API failure, so if we reach
              # the comment call the update succeeded. Keep these in sequence —
              # do not refactor cmd_task_update to return an error code without
              # also adding an explicit success check here.
              rc = cmd_task_update([task_id, "--done"])
              if "comment" in flags:
                  cmd_task_comment([task_id, "--body", flags["comment"]])
              return rc

          if sub == "note":
              flags, positional = _parse_args(rest, {})
              if len(positional) < 2:
                  print('Usage: vikunja team-task note <id> "<comment>"',
                        file=sys.stderr)
                  sys.exit(2)
              task_id = positional[0]
              # Join remaining positionals so unquoted multi-word comments still
              # work instead of silently truncating to the first token.
              body = " ".join(positional[1:])
              return cmd_task_comment([task_id, "--body", body])

          print(f"Unknown team-task subcommand: {sub}", file=sys.stderr)
          sys.exit(2)
  ```

- [ ] **Step 5: Register in the main dispatcher**

  In `main()`, add after the `my-task` branches:

  ```python
      elif cmd == "team-tasks":
          return cmd_team_tasks(rest)
      elif cmd == "team-task":
          return cmd_team_task(rest)
  ```

- [ ] **Step 6: Add help entries**

  In `COMMAND_HELP`:

  ```python
      "team-tasks": (
          "Usage: vikunja team-tasks [--open] "
          "[--sort position|priority|due] [--format text|json]\n"
          "List tasks in the Sam-Walter shared project "
          "(VIKUNJA_TEAM_PROJECT_ID).\n"
      ),
      "team-task": (
          "Usage: vikunja team-task {create|show|done|note} <args>\n"
          "Operate on one task in the shared Sam-Walter project.\n"
          "  create <title> [--description ...] [--priority 1-5] [--due YYYY-MM-DD]\n"
          "  show   <id>\n"
          "  done   <id> [--comment \"...\"]\n"
          "  note   <id> <comment>\n"
      ),
  ```

  Also update the top-level module docstring to include `team-*` next to `my-*`.

- [ ] **Step 7: Validate**

  ```bash
  python3 -c "
  import yaml
  with open('/home/wsl2user/github_projects/zeroclaw/k8s/sam/20_vikunja_tool_configmap.yaml') as f:
      d = yaml.safe_load(f)
  script = d['data']['vikunja']
  compile(script, 'vikunja', 'exec')
  print('YAML + Python OK')
  "
  ```

  Expected: `YAML + Python OK`.

- [ ] **Step 8: Commit (only this file)**

  Do NOT stage any of the pre-existing dirty files (`03_zeroclaw_configmap.yaml`, `04_zeroclaw_sandbox.yaml`, `22_science_curator_skill_configmap.yaml`, or any others that are dirty at the time of this plan's execution). Use targeted `git add`.

  ```bash
  cd /home/wsl2user/github_projects/zeroclaw
  git add k8s/sam/20_vikunja_tool_configmap.yaml
  git commit -m "$(cat <<'EOF'
  feat(vikunja-cli): add team-tasks / team-task commands

  Parallels the existing my-* commands but uses VIKUNJA_TEAM_PROJECT_ID
  instead of VIKUNJA_SAM_PROJECT_ID, for the shared Sam-Walter board
  (project #5 in this cluster). Both agents mount the same CLI; this
  command set is the surface both sides use to coordinate.

  Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 3: Create the `team-coordination` skill ConfigMap

**Files:**
- Create: `k8s/shared/27_team_coordination_skill_configmap.yaml`

The skill applies the v1.2.0 lesson from `task-tracking` — `always: true` from the start. Both Sam and Walter mount this same ConfigMap; the skill body contains both sides of the flow so the rules stay in sync.

- [ ] **Step 1: Create the directory**

  ```bash
  mkdir -p /home/wsl2user/github_projects/zeroclaw/k8s/shared
  ```

- [ ] **Step 2: Write the ConfigMap file**

  Create `/home/wsl2user/github_projects/zeroclaw/k8s/shared/27_team_coordination_skill_configmap.yaml` with this exact content:

  ```yaml
  apiVersion: v1
  kind: ConfigMap
  metadata:
    name: zeroclaw-skill-team-coordination
    namespace: ai-agents
    labels:
      app: zeroclaw
      component: skill
  data:
    team-coordination.md: |
      ---
      name: team-coordination
      version: 1.0.0
      description: >
        Coordination surface between Sam and Walter via Vikunja project #5
        (Sam & Walter ops). Always resident — consult on every session
        start (Walter) or every Dan ask that needs code/k8s work (Sam).
      always: true
      ---

      # Team Coordination

      You share a Vikunja project with your counterpart:
      - **Sam** is Signal-facing; she takes asks from Dan and delegates
        code/k8s work to Walter.
      - **Walter** is Gitea/k8s-facing; he runs via ACP (`acp-client`)
        from Sam, ships PRs, and reports results back.

      The shared board (`VIKUNJA_TEAM_PROJECT_ID`, project #5 in this
      cluster) is your durable handoff surface. The task description is
      the work spec. The close-comment is the findings. Both sides can
      audit the history at any time.

      ## If you are Sam

      **When Dan asks you for something that needs code / k8s / PR work:**

      1. Create the parent task in YOUR inbox with `vikunja my-task create
         ...` (as usual — that's task-tracking skill's domain).
      2. Create a DELEGATION task in the shared board:
         `vikunja team-task create "<spec for Walter>" --description "<full
         context: what, why, acceptance criteria, links to Dan's message
         if helpful>"`. Capture the team-task ID.
      3. Dispatch Walter via ACP:
         `acp-client send "<brief instruction + team-task #N>"`.
         Walter will start, read task #N, do the work, close it.
      4. Poll: `acp-client poll <session_id>` every 30-60s until
         COMPLETE, and `vikunja team-task show <N>` after completion to
         read Walter's close-comment.
      5. Note Walter's PR URL / findings on your OWN task
         (`vikunja my-task note <parent-id> "Walter finished in
         team-task #N, PR: https://..."`), then close your parent task
         and reply to Dan.

      **When to delegate to Walter vs. do it yourself:** if the work
      produces a PR, touches the cluster, or requires k8s/gitea tooling
      Walter has (and you don't), delegate. Short research, wiki edits,
      Vikunja work, and Signal-facing replies stay with you.

      ## If you are Walter

      **At the start of EVERY ACP session, your first action is:**

      ```
      vikunja team-tasks --open
      ```

      Usually there's one newly-created task matching Sam's message.
      If Sam's message references a specific task ID ("team-task #42"),
      read that one. If multiple open tasks exist and it's unclear which
      Sam means, pick the most recent and note your choice in a comment
      so Sam can correct if wrong.

      **Workflow:**

      1. `vikunja team-task show <id>` — read the description (spec).
      2. Do the work. This usually ends in a PR on Gitea.
      3. `vikunja team-task done <id> --comment "<findings + PR URL +
         any caveats>"`. This is what Sam reads when relaying to Dan.
      4. Reply via ACP with a short summary. The comment has detail;
         the ACP reply is a pointer.

      **Upward escalation:** if you discover something outside the
      delegated task that needs Sam's or Dan's attention (e.g., a
      flaky deployment, a missing permission, a stale dependency),
      open a NEW team-task yourself:

      ```
      vikunja team-task create "<concise ask>" --description "<what you
      found, why it matters, who should look at it>"
      ```

      Sam sees it on her next team-tasks check. Don't escalate
      inside the close-comment of the task you were given — that
      gets lost. Separate tasks keep separate concerns.

      ## Worked example

      Dan → Sam (Signal): *"Can you get Walter to close out the stale
      zeroclaw-sam tag in Gitea — there's a v1.5.0 without notes."*

      Sam:
      ```
      $ vikunja my-task create "Close stale zeroclaw-sam:v1.5.0 tag note" \
          --description "Dan wants a release note or proper deletion"
      Task #127 created
      $ vikunja team-task create "Audit Gitea for missing release notes on zeroclaw-sam tags" \
          --description "Dan noticed v1.5.0 has no release notes. Either add one, or deprecate the tag. Preferred: add notes referencing the nearest commit."
      Task #128 created
      $ acp-client send "Team-task #128 — see description. Take your time."
      PENDING acp_session_id=abc123
      # ... poll every 60s ...
      $ acp-client poll abc123
      COMPLETE
      $ vikunja team-task show 128
      [DONE] with comment: "Added release notes for v1.5.0 via PR gitea.example/zeroclaw/pulls/42. Tagged commit 2f071808."
      $ vikunja my-task note 127 "Walter closed team-task #128: PR #42 merged"
      $ vikunja my-task done 127 --comment "PR merged: https://gitea.example/zeroclaw/pulls/42"
      Reply to Dan: "Done. Walter added release notes; PR 42 merged."
      ```

      Walter (inside the ACP session for team-task #128):
      ```
      $ vikunja team-tasks --open
        #128 [TODO] Audit Gitea for missing release notes ...
      $ vikunja team-task show 128
      [... reads description ...]
      # does the work on Gitea, opens PR #42
      $ vikunja team-task done 128 \
          --comment "Added release notes for v1.5.0 via PR gitea.example/zeroclaw/pulls/42. Tagged commit 2f071808."
      # replies to ACP: "PR #42 opened with release notes for v1.5.0."
      ```

      ## CLI reference

      Both agents use the same surface:

      ```
      vikunja team-tasks [--open] [--format text|json]
      vikunja team-task create "<title>" [--description ...] [--priority 1-5] [--due YYYY-MM-DD]
      vikunja team-task show   <id>
      vikunja team-task done   <id> [--comment "..."]
      vikunja team-task note   <id> "<progress>"
      ```

      Project ID is baked in via `VIKUNJA_TEAM_PROJECT_ID`.

      ## Failure modes

      - ACP session timing out while Walter is still working → Sam
        should `acp-client poll` again; Walter's progress is in the
        task's comments/notes, not the ACP session buffer.
      - Walter starts a session with no open team-tasks → probably a
        stale ACP retry; reply "no task in team board — please
        re-delegate if this is new work" and end the session.
      - Task title collision (two tasks sound similar) → use the
        numeric ID Sam references in the ACP message; the ID is the
        source of truth, the title is a human hint.
      - PR merge is blocked (CI, review) → Walter closes the
        team-task with `--comment "PR opened but merge blocked: <why>.
        Needs Sam/Dan."` rather than leaving it open — closing with
        an explicit blocker is cleaner than a stale TODO.
  ```

- [ ] **Step 3: Validate YAML**

  ```bash
  python3 -c "
  import yaml
  d = yaml.safe_load(open('/home/wsl2user/github_projects/zeroclaw/k8s/shared/27_team_coordination_skill_configmap.yaml'))
  c = d['data']['team-coordination.md']
  assert 'version: 1.0.0' in c
  assert 'always: true' in c
  assert 'team-task create' in c
  assert 'acp-client' in c
  print(f'OK - {len(c)} chars, ~{len(c)//4} tokens')
  "
  ```

  Expected: `OK - <N> chars` where N is ~4500-5500.

- [ ] **Step 4: Commit**

  ```bash
  cd /home/wsl2user/github_projects/zeroclaw
  git add k8s/shared/27_team_coordination_skill_configmap.yaml
  git commit -m "$(cat <<'EOF'
  feat(k8s): add team-coordination skill for Sam-Walter handoff

  Shared ConfigMap mounted on both Sam and Walter. always: true from
  the start (applying the v1.2.0 lesson from task-tracking — behavior
  skills must be resident). Body contains both sides of the flow so
  the rules stay in sync between the two agents.

  Depends on:
  - VIKUNJA_TEAM_PROJECT_ID env (set on both pods in follow-up commits)
  - Vikunja project #5 (Sam & Walter ops) with both agent users as members
  - team-tasks / team-task CLI commands (this plan's Task 2)
  - acp-client already present on Sam for the delegation trigger

  Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 4: Sam — env var + skill mount

**Files:**
- Modify: `k8s/sam/03_zeroclaw_configmap.yaml` (add to `shell_env_passthrough`)
- Modify: `k8s/sam/04_zeroclaw_sandbox.yaml` (add env + mount)

### Sam's dirty-file caveat

At plan execution time, `04_zeroclaw_sandbox.yaml` and `03_zeroclaw_configmap.yaml` likely still have pre-existing uncommitted changes from the science-curator v3.0.0 work. Use `git add -p` to stage only the hunks this task creates; never stage the science-curator hunks.

- [ ] **Step 1: Add the passthrough entry**

  In `03_zeroclaw_configmap.yaml`, line 67-ish, append `VIKUNJA_TEAM_PROJECT_ID` to `shell_env_passthrough`:

  Before:
  ```toml
  shell_env_passthrough = ["ACP_AUTH_TOKEN", "ACP_BASE_URL", "ACP_CWD", "SPEAKR_API_TOKEN", "VIKUNJA_API_TOKEN", "VIKUNJA_BASE_URL", "VIKUNJA_SAM_PROJECT_ID"]
  ```

  After:
  ```toml
  shell_env_passthrough = ["ACP_AUTH_TOKEN", "ACP_BASE_URL", "ACP_CWD", "SPEAKR_API_TOKEN", "VIKUNJA_API_TOKEN", "VIKUNJA_BASE_URL", "VIKUNJA_SAM_PROJECT_ID", "VIKUNJA_TEAM_PROJECT_ID"]
  ```

- [ ] **Step 2: Add the env var to Sam's sandbox**

  In `04_zeroclaw_sandbox.yaml`, after the `VIKUNJA_SAM_PROJECT_ID` block (~line 159-160):

  ```yaml
              - name: VIKUNJA_TEAM_PROJECT_ID
                value: "5"
  ```

  Match surrounding indentation (12 spaces).

- [ ] **Step 3: Add the skill mount**

  In the same file, find the existing `skill-task-tracking` mount block and add an analogous block for `skill-team-coordination` immediately after:

  ```yaml
              - name: skill-team-coordination
                mountPath: /data/.zeroclaw/workspace/skills/team-coordination/SKILL.md
                subPath: team-coordination.md
                readOnly: true
  ```

- [ ] **Step 4: Add the volume reference**

  In the same file, find `skill-task-tracking` in the volumes block and add:

  ```yaml
          - name: skill-team-coordination
            configMap:
              name: zeroclaw-skill-team-coordination
  ```

- [ ] **Step 5: Add the mkdir**

  In the init container command, after the existing `mkdir -p ...skills/task-tracking`:

  ```bash
                mkdir -p /data/.zeroclaw/workspace/skills/team-coordination
  ```

- [ ] **Step 6: Validate**

  ```bash
  python3 -c "
  import yaml
  for f in [
      '/home/wsl2user/github_projects/zeroclaw/k8s/sam/03_zeroclaw_configmap.yaml',
      '/home/wsl2user/github_projects/zeroclaw/k8s/sam/04_zeroclaw_sandbox.yaml',
  ]:
      list(yaml.safe_load_all(open(f)))
  print('YAML OK')
  "
  ```

- [ ] **Step 7: Stage with `git add -p` to exclude dirty hunks; commit**

  ```bash
  cd /home/wsl2user/github_projects/zeroclaw
  # 03_zeroclaw_configmap.yaml: stage only the passthrough hunk, skip science-curator reliability hunks
  # 04_zeroclaw_sandbox.yaml: stage only the env + mount + volume + mkdir hunks, skip science-curator mount hunks
  git add -p k8s/sam/03_zeroclaw_configmap.yaml  # answer y for passthrough, n for science-curator
  git add -p k8s/sam/04_zeroclaw_sandbox.yaml    # answer y/n selectively
  git diff --cached  # REVIEW — must show only this task's changes
  git commit -m "feat(sam): wire VIKUNJA_TEAM_PROJECT_ID + team-coordination mount"
  ```

---

## Task 5: Walter — image bump + Vikunja env + CLI mount + skill mount

**Files:**
- Modify: `k8s/walter/01_configmap.yaml` (add passthrough)
- Modify: `k8s/walter/03_sandbox.yaml` (image, env, CLI mount, skill mount, init script)

Walter's changes are grouped because they all live in his two files and must deploy together.

- [ ] **Step 1: Inventory Walter's config and sandbox shapes**

  Read `k8s/walter/01_configmap.yaml` to confirm it has a `shell_env_passthrough` directive; if yes, append the new vars; if no, stop and report — the pattern may be different and needs a design decision before proceeding.

  Read `k8s/walter/03_sandbox.yaml` fully to identify:
  - The image tag line
  - The env block
  - The volumeMounts block for the zeroclaw container
  - The volumes block at the pod level
  - The init container command script (where the existing `cp /etc/zeroclaw-template/skills/*.md ...` loop lives)

- [ ] **Step 2: Bump the image**

  In `03_sandbox.yaml`, change the two occurrences of `citizendaniel/zeroclaw-sam:v1.4.12` (or whatever the current tag is) to `citizendaniel/zeroclaw-sam:v1.5.15`.

- [ ] **Step 3: Add Vikunja env vars**

  In the env block, after `GITEA_API_TOKEN` (or anywhere after `API_KEY`):

  ```yaml
              - name: VIKUNJA_API_TOKEN
                valueFrom:
                  secretKeyRef:
                    name: zeroclaw-k8s-agent-secrets
                    key: vikunja-api-token
                    optional: false
              - name: VIKUNJA_BASE_URL
                value: "http://vikunja.todolist.svc.cluster.local:3456"
              - name: VIKUNJA_TEAM_PROJECT_ID
                value: "5"
  ```

  The `optional: false` is deliberate — if Vault isn't set up (Task 1 not done), Walter's pod will FailToStart with a clear error, which is better than a silent 401 on every Vikunja call.

- [ ] **Step 4: Add the vikunja CLI mount**

  In the zeroclaw container's volumeMounts:

  ```yaml
              - name: vikunja-tool
                mountPath: /usr/local/bin/vikunja
                subPath: vikunja
                readOnly: true
  ```

  And at the pod volumes level:

  ```yaml
          - name: vikunja-tool
            configMap:
              name: zeroclaw-vikunja-tool
              defaultMode: 0755
              items:
                - key: vikunja
                  path: vikunja
                  mode: 0755
  ```

  The `defaultMode: 0755` + `items[].mode: 0755` matches Sam's mount of the same ConfigMap (scripts need to be executable).

- [ ] **Step 5: Add the team-coordination skill mount**

  Walter's skill discovery iterates `/etc/zeroclaw-template/skills/*.md`. Add a second source directory by mounting the new ConfigMap there:

  In volumeMounts:

  ```yaml
              - name: skills-extra
                mountPath: /etc/zeroclaw-template/skills-extra
                readOnly: true
  ```

  At pod volumes:

  ```yaml
          - name: skills-extra
            configMap:
              name: zeroclaw-skill-team-coordination
  ```

- [ ] **Step 6: Extend the init script to copy from `skills-extra`**

  Locate the existing skill-loading loop in the init container command (the one that does `for f in /etc/zeroclaw-template/skills/*.md`). Add an analogous loop for the extra directory immediately after it:

  Before:
  ```bash
                for f in /etc/zeroclaw-template/skills/*.md; do
                  skill_name=$(basename "$f" .md)
                  mkdir -p "/data/.zeroclaw/workspace/skills/$skill_name"
                  cp "$f" "/data/.zeroclaw/workspace/skills/$skill_name/SKILL.md"
                done
  ```

  After (add this block right after):
  ```bash
                # Extra skills from separate ConfigMaps (e.g., team-coordination,
                # shared with Sam). Same copy pattern.
                if [ -d /etc/zeroclaw-template/skills-extra ]; then
                  for f in /etc/zeroclaw-template/skills-extra/*.md; do
                    [ -f "$f" ] || continue
                    skill_name=$(basename "$f" .md)
                    mkdir -p "/data/.zeroclaw/workspace/skills/$skill_name"
                    cp "$f" "/data/.zeroclaw/workspace/skills/$skill_name/SKILL.md"
                  done
                fi
  ```

- [ ] **Step 7: Update passthrough in `01_configmap.yaml`**

  If Walter's `shell_env_passthrough` exists and currently includes `ACP_AUTH_TOKEN` etc., append `VIKUNJA_API_TOKEN`, `VIKUNJA_BASE_URL`, and `VIKUNJA_TEAM_PROJECT_ID` to that list so the CLI invoked via `shell:` sees them.

  If the directive doesn't exist in Walter's config, add it next to a similar `[autonomy]`-block setting:

  ```toml
  shell_env_passthrough = ["ACP_AUTH_TOKEN", "GITEA_API_TOKEN", "VIKUNJA_API_TOKEN", "VIKUNJA_BASE_URL", "VIKUNJA_TEAM_PROJECT_ID"]
  ```

- [ ] **Step 8: Validate all Walter YAML**

  ```bash
  python3 -c "
  import yaml
  for f in [
      '/home/wsl2user/github_projects/zeroclaw/k8s/walter/01_configmap.yaml',
      '/home/wsl2user/github_projects/zeroclaw/k8s/walter/03_sandbox.yaml',
  ]:
      list(yaml.safe_load_all(open(f)))
      print(f, 'OK')
  "
  ```

- [ ] **Step 9: Commit**

  Walter's files are probably clean (the dirty tree is Sam-side). Verify with `git status` first. Then:

  ```bash
  cd /home/wsl2user/github_projects/zeroclaw
  git add k8s/walter/01_configmap.yaml k8s/walter/03_sandbox.yaml
  git commit -m "$(cat <<'EOF'
  feat(walter): bump to v1.5.15 and wire Vikunja team-coordination

  - Bump image v1.5.0 → v1.5.15 (catches up on isolation-leak fix
    (cosmetic for Walter — his only cron is shell-type), Gemma 4
    upstream sync, configurable provider timeout, compact skills).
  - Add VIKUNJA_API_TOKEN from Vault-backed secret (walter user
    token), VIKUNJA_BASE_URL, and VIKUNJA_TEAM_PROJECT_ID=5.
  - Mount the zeroclaw-vikunja-tool ConfigMap as /usr/local/bin/vikunja
    so Walter has the same CLI surface as Sam.
  - Mount the new zeroclaw-skill-team-coordination ConfigMap at
    /etc/zeroclaw-template/skills-extra and extend the init script
    to iterate that directory alongside the primary skills mount.

  Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
  EOF
  )"
  ```

---

## Task 6: Apply and roll

**Files:** None modified.

Order matters — apply ConfigMaps first so the pods see the new content when they come up.

- [ ] **Step 1: Confirm prerequisites (Task 1)**

  ```bash
  # Vault-backed secret must already have the Walter Vikunja token
  kubectl get secret -n ai-agents zeroclaw-k8s-agent-secrets -o jsonpath='{.data.vikunja-api-token}' | base64 -d | head -c 10; echo
  ```

  Expected: the first ~10 chars of the Walter API token. If empty, STOP — Task 1 isn't done; Walter's pod would FailToStart.

- [ ] **Step 2: Apply shared skill ConfigMap**

  ```bash
  kubectl apply -f /home/wsl2user/github_projects/zeroclaw/k8s/shared/27_team_coordination_skill_configmap.yaml
  ```

  Expected: `configmap/zeroclaw-skill-team-coordination created`.

- [ ] **Step 3: Apply vikunja CLI ConfigMap**

  ```bash
  kubectl apply -f /home/wsl2user/github_projects/zeroclaw/k8s/sam/20_vikunja_tool_configmap.yaml
  ```

  Expected: `configmap/zeroclaw-vikunja-tool configured`.

- [ ] **Step 4: Apply Sam config + sandbox**

  ```bash
  # Stash any remaining dirty hunks so the applied state matches committed state
  cd /home/wsl2user/github_projects/zeroclaw
  git stash push k8s/sam/03_zeroclaw_configmap.yaml k8s/sam/04_zeroclaw_sandbox.yaml 2>&1 | tail -3

  kubectl apply -f k8s/sam/03_zeroclaw_configmap.yaml
  kubectl apply -f k8s/sam/04_zeroclaw_sandbox.yaml

  git stash pop
  ```

- [ ] **Step 5: Apply Walter config + sandbox**

  Walter's files should be clean after Task 5. If not, stash first.

  ```bash
  kubectl apply -f /home/wsl2user/github_projects/zeroclaw/k8s/walter/01_configmap.yaml
  kubectl apply -f /home/wsl2user/github_projects/zeroclaw/k8s/walter/03_sandbox.yaml
  ```

- [ ] **Step 6: Roll both pods**

  ```bash
  kubectl delete pod -n ai-agents zeroclaw
  kubectl delete pod -n ai-agents zeroclaw-k8s-agent
  ```

- [ ] **Step 7: Wait for both pods**

  ```bash
  kubectl get pods -n ai-agents -w
  ```

  Wait until both `zeroclaw` and `zeroclaw-k8s-agent` show `2/2 Running`. If either stays in `CrashLoopBackOff`, check `kubectl describe pod` and `kubectl logs` — most likely suspect is a missing Vault key (401s on startup health check) or a YAML typo.

- [ ] **Step 8: Verify mounts and env**

  Sam:
  ```bash
  kubectl exec -n ai-agents zeroclaw -c zeroclaw -- env | grep VIKUNJA_TEAM
  kubectl exec -n ai-agents zeroclaw -c zeroclaw -- cat /data/.zeroclaw/workspace/skills/team-coordination/SKILL.md | head -5
  kubectl exec -n ai-agents zeroclaw -c zeroclaw -- vikunja team-tasks
  ```

  Expected:
  - `VIKUNJA_TEAM_PROJECT_ID=5`
  - Frontmatter with `name: team-coordination`, `version: 1.0.0`
  - `No tasks in project #5.` (assuming fresh board)

  Walter:
  ```bash
  kubectl exec -n ai-agents zeroclaw-k8s-agent -c zeroclaw -- env | grep VIKUNJA
  kubectl exec -n ai-agents zeroclaw-k8s-agent -c zeroclaw -- cat /data/.zeroclaw/workspace/skills/team-coordination/SKILL.md | head -5
  kubectl exec -n ai-agents zeroclaw-k8s-agent -c zeroclaw -- vikunja team-tasks
  ```

  Expected: same three outputs.

---

## Task 7: End-to-end smoke test

**Files:** None modified.

- [ ] **Step 1: Synthetic round-trip from Sam's pod (no ACP)**

  Verify both agents can read/write the shared board from their own pods:

  ```bash
  kubectl exec -n ai-agents zeroclaw -c zeroclaw -- vikunja team-task create 'ZZ smoke — delete me' --description 'smoke test from sam pod'
  # record the ID, call it $ID
  kubectl exec -n ai-agents zeroclaw-k8s-agent -c zeroclaw -- vikunja team-task show $ID
  # expected: same task body, visible to walter
  kubectl exec -n ai-agents zeroclaw-k8s-agent -c zeroclaw -- vikunja team-task done $ID --comment 'smoke close from walter pod'
  kubectl exec -n ai-agents zeroclaw -c zeroclaw -- vikunja team-task show $ID
  # expected: [DONE], comment visible
  kubectl exec -n ai-agents zeroclaw -c zeroclaw -- vikunja team-task show $ID | grep '@walter'
  # expected: the comment is attributed to @walter (not @sam) — confirms separate token
  kubectl exec -n ai-agents zeroclaw -c zeroclaw -- vikunja task delete $ID
  ```

- [ ] **Step 2: Real delegation test via ACP (Dan-in-loop)**

  Ask Dan to send Sam a Signal message that requires Walter, e.g.:

  > Hey Sam, can you have Walter audit the Gitea repos and report back which ones have no release notes in the last 3 months? No deadline.

  Then observe (under 5 min):
  - Sam creates a task in her inbox (`my-tasks`) — task-tracking v1.2.0 should fire
  - Sam creates a delegation task in the team board (`team-tasks`) — team-coordination should fire
  - Sam invokes `acp-client send` with a reference to the team-task
  - Walter's session starts, Walter runs `vikunja team-tasks --open` first (per the skill)
  - Walter does the work, closes the team-task with findings
  - Sam polls, reads Walter's close-comment, replies to Dan

  Expected outcome: all four Vikunja task-state transitions happen in the correct order (parent TODO → team TODO → team DONE → parent DONE).

- [ ] **Step 3: If any step fails**

  Report which step, what Sam's or Walter's log said, and whether the failure is:
  - Skill-text calibration (Sam/Walter didn't follow the rules → tune skill body, iterate like task-tracking v1.2.0)
  - CLI bug (team-task command failed with a Python error → fix CLI, re-apply ConfigMap, re-roll pod)
  - Integration bug (ACP didn't fire, Vikunja 403, Vault missing) → surface to Dan

---

## Task 8: Document in the scrapyard wiki

**Files:**
- Modify: `~/github_projects/scrapyard-wiki/wiki/services/zeroclaw.md`
- Modify: `~/github_projects/scrapyard-wiki/wiki/services/zeroclaw-k8s-agent.md`
- Modify: `~/github_projects/scrapyard-wiki/log.md`

- [ ] **Step 1: Update `services/zeroclaw.md`**

  In the Skills list, add:

  ```markdown
  - team-coordination (v1.0.0 — see below, shared with [[services/zeroclaw-k8s-agent]])
  ```

  Add a subsection under Custom Fork Changes describing the new skill and CLI surface, pointing to the shared ConfigMap path.

- [ ] **Step 2: Update `services/zeroclaw-k8s-agent.md`**

  Add a Skills section (or extend an existing one) documenting that Walter now has `team-coordination` + the vikunja CLI, with the mount pattern (second ConfigMap at `skills-extra` + extended init script).

- [ ] **Step 3: Log the ingest**

  Append a dated entry to `log.md`:

  ```markdown
  ## [2026-04-16] ingest | team-coordination skill + Walter v1.5.15 bump
  - Created: [[services/zeroclaw-k8s-agent]] skill section
  - Updated: [[services/zeroclaw]] skill list + subsection
  - New Vikunja project #5 Sam & Walter ops; Walter user + API token added to Vault
  ```

- [ ] **Step 4: Commit**

  ```bash
  cd /home/wsl2user/github_projects/scrapyard-wiki
  git add wiki/services/zeroclaw.md wiki/services/zeroclaw-k8s-agent.md log.md
  git commit -m "wiki: document team-coordination skill (v1.0.0) + Walter v1.5.15"
  ```

---

## Self-review checklist for the engineer

- [ ] All 8 tasks marked complete
- [ ] Real delegation round-trip succeeded end-to-end (Task 7 Step 2)
- [ ] `@walter` attribution appears in team-task comments from Walter, `@sam` (or `@dan`) from Sam (confirms separate tokens)
- [ ] Both pods at `v1.5.15`
- [ ] No hardcoded `5` in the CLI — only `TEAM_PROJECT_ID` / env reads
- [ ] `vikunja team-task --help` prints the new commands on both pods
- [ ] No pre-existing science-curator hunks accidentally committed
- [ ] Skill's decision rules are mirrored on both sides (Sam's "delegate" rules + Walter's "receive" rules), not duplicated with drift

## Non-goals (out of scope)

- A native Rust tool for team-task ops (defer until shell+CLI demonstrates friction)
- Retroactive migration of prior Sam→Walter handoffs into Vikunja
- Auto-creating a team-task when Sam uses `acp-client send` without first creating one (nice-to-have; needs a hook in zeroclaw core, not a skill)
- Per-user read-only audit views (Dan can already see everything in Vikunja)
- Slack/Telegram announce delivery on close (not needed — Sam relays via Signal; Walter doesn't need delivery)
