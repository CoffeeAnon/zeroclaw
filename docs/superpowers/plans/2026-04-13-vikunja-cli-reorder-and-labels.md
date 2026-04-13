# Vikunja CLI: Reorder, Labels, Delete, Show, and Per-Command Help — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend Sam's `vikunja` CLI (a Python script mounted via ConfigMap) with five new subcommands so she can reorder tasks, manage labels, delete duplicates, show task detail, and discover usage via per-command help.

**Architecture:** Edit the existing `vikunja.py` embedded in `k8s/sam/20_vikunja_tool_configmap.yaml` in place. Validate syntax between steps by extracting the embedded script to `/tmp/vikunja.py` and running `python3 -m py_compile`. Deploy once at the end (Task 8) with a single ConfigMap apply + pod restart, then run each new command end-to-end against the real Vikunja instance. Follow the existing stdlib-only pattern (no new deps).

**Tech Stack:** Python 3 (stdlib only: `urllib.request`, `urllib.parse`, `json`, `os`, `sys`), Kubernetes ConfigMap, Vikunja REST API v1.

**Vikunja API endpoints used (new):**
- `POST /api/v1/tasks/{id}/position` — body `{"project_view_id": N, "position": float}` — reorder a task within a list view
- `GET /api/v1/labels[?s=name]` — list/search labels
- `PUT /api/v1/labels` — create a label (body: `{title, hex_color, description}`)
- `PUT /api/v1/tasks/{id}/labels` — attach a label to a task (body: `{label_id}`)
- `DELETE /api/v1/tasks/{id}/labels/{labelId}` — detach a label
- `DELETE /api/v1/tasks/{id}` — delete a task

**Context — what's broken today:**
- `vikunja --help` lists `project create` and `task create/update/assign/comment` but gives no per-command help, so calling `vikunja task create --help` just errors about missing `--title`.
- There is no way to reorder tasks. Sam had to guess at "priority" and couldn't affect the list view ordering.
- There is no way to create/attach labels. Sam only has the integer `priority` field (1-5), which is not visible enough in the UI for her to use as a priority signal.
- There is no way to delete a task, which blocks removing duplicates.
- There is no detail view — the list output shows a one-line summary with no labels, no description, and no comments. The new `task show` command closes all three gaps, including fetching comments via a second API call to `/api/v1/tasks/{id}/comments`.

**Helper commands you'll use repeatedly** (copy-paste these into a scratch file to save typing):

```bash
# Extract the embedded Python from the ConfigMap YAML for syntax checking / local runs.
extract_vikunja() {
  python3 -c "
import yaml
data = yaml.safe_load(open('k8s/sam/20_vikunja_tool_configmap.yaml'))
open('/tmp/vikunja.py', 'w').write(data['data']['vikunja.py'])
print('Extracted to /tmp/vikunja.py')
"
  chmod +x /tmp/vikunja.py
}

# Syntax-check after each edit.
check_vikunja() {
  extract_vikunja && python3 -m py_compile /tmp/vikunja.py && echo "OK: syntax clean"
}
```

**Existing code structure (reference):** `k8s/sam/20_vikunja_tool_configmap.yaml` contains one `data.vikunja.py` string. All edits in this plan are to that one key. Preserve the 4-space indentation inside the YAML block scalar (the script body is indented 4 spaces so `yaml.safe_load` returns the unindented Python).

---

## Task 1: Per-Command Help Infrastructure

**Files:**
- Modify: `k8s/sam/20_vikunja_tool_configmap.yaml`

Add a `COMMAND_HELP` dict, a `print_command_help()` helper, a `_check_help()` early-exit helper, and route `vikunja help <cmd>` / `vikunja <cmd> --help` / `vikunja <cmd> help` to it. Then wire `_check_help` into every existing command function so each one responds to `--help`/`-h`/`help`.

- [ ] **Step 1: Extract current script and confirm baseline syntax**

```bash
cd /home/wsl2user/github_projects/zeroclaw
python3 -c "
import yaml
data = yaml.safe_load(open('k8s/sam/20_vikunja_tool_configmap.yaml'))
open('/tmp/vikunja.py', 'w').write(data['data']['vikunja.py'])
"
python3 -m py_compile /tmp/vikunja.py && echo OK
```

Expected: `OK`.

- [ ] **Step 2: Add `COMMAND_HELP` table and helpers after the `_parse_args` function**

Find the existing `def _parse_args(args, flags):` block in `k8s/sam/20_vikunja_tool_configmap.yaml` (ends with `return parsed, positional`). Insert the following block immediately after that function (keep the 4-space indent that exists inside the YAML block scalar):

```python
    COMMAND_HELP = {
        "projects": (
            'Usage: vikunja projects\n'
            '\n'
            '  List all projects you have access to.\n'
        ),
        "project create": (
            'Usage: vikunja project create --title "..." [--description "..."]\n'
            '\n'
            '  Create a new project.\n'
        ),
        "tasks": (
            'Usage: vikunja tasks <project-id> [--sort position|priority|due] [--format text|json]\n'
            '\n'
            '  List tasks in a project.\n'
            '\n'
            'Flags:\n'
            '  --sort FIELD     position (default, respects manual reorder), priority (highest first), due (earliest first)\n'
            '  --format FORMAT  text (default) or json (machine-readable, full task records)\n'
        ),
        "tasks reorder": (
            'Usage: vikunja tasks reorder <project-id> --order "id1,id2,id3,..."\n'
            '\n'
            '  Resequence every task in a project so the IDs you pass appear\n'
            '  first, in the order you gave them, and any tasks you did NOT\n'
            '  mention keep their relative order but are placed after the\n'
            '  explicit block. Every task in the view gets a fresh position,\n'
            '  so the result is deterministic regardless of prior state.\n'
            '\n'
            '  All IDs in --order must belong to the project, must be unique,\n'
            '  and must be visible in the default list view.\n'
            '\n'
            'Example:\n'
            '  vikunja tasks reorder 4 --order "17,23,19,12"\n'
        ),
        "task create": (
            'Usage: vikunja task create <project-id> --title "..." \\\n'
            '         [--description "..."] [--due "YYYY-MM-DD"] [--priority 1-5] [--assignee <username>]\n'
            '\n'
            '  Create a new task in a project. --priority is the native integer\n'
            '  priority field (1=lowest, 5=highest). For visual priority labels\n'
            '  use `vikunja task label`.\n'
        ),
        "task update": (
            'Usage: vikunja task update <task-id> [--done] [--title "..."] \\\n'
            '         [--description "..."] [--due "YYYY-MM-DD"] [--priority 1-5] [--assignee <username>]\n'
            '\n'
            '  Update an existing task. Any omitted field is left unchanged.\n'
        ),
        "task show": (
            'Usage: vikunja task show <task-id>\n'
            '\n'
            '  Show full details for a task: title, description, priority,\n'
            '  labels, due date, assignees, and comments.\n'
        ),
        "task delete": (
            'Usage: vikunja task delete <task-id>\n'
            '\n'
            '  Delete a task permanently. Use this to remove duplicates.\n'
        ),
        "task assign": (
            'Usage: vikunja task assign <task-id> --user <username>\n'
            '\n'
            '  Assign a user to a task.\n'
        ),
        "task comment": (
            'Usage: vikunja task comment <task-id> --body "..."\n'
            '\n'
            '  Add a comment to a task.\n'
        ),
        "task label": (
            'Usage:\n'
            '  vikunja task label <task-id> --list\n'
            '  vikunja task label <task-id> --add "name" [--create-missing] [--color ff0000]\n'
            '  vikunja task label <task-id> --remove "name"\n'
            '\n'
            '  Manage labels on a task. Labels are looked up by name (exact match).\n'
            '  With --create-missing, a label that does not yet exist is created\n'
            '  automatically. --color is a 6-char hex code (no leading #).\n'
        ),
        "labels": (
            'Usage: vikunja labels [--search "..."]\n'
            '\n'
            '  List all labels you have access to. --search filters by name.\n'
        ),
        "label create": (
            'Usage: vikunja label create --title "..." [--color "ff0000"] [--description "..."]\n'
            '\n'
            '  Create a new label. --color is a 6-char hex code without the leading #.\n'
        ),
    }


    def print_command_help(command_path):
        """Print help for a specific command path (e.g., 'task create')."""
        text = COMMAND_HELP.get(command_path)
        if text:
            print(text)
        else:
            print(f"No help available for: {command_path}\n")
            print(__doc__)


    def _check_help(args, command_path):
        """If args requests help, print command help and exit 0.

        Matches either the literal 'help' as the first positional arg, or
        '--help'/'-h' anywhere. Not matching bare 'help' mid-arg-list avoids
        false positives when a user passes 'help' as a flag value, e.g.
        `task create --title help`.
        """
        if args and args[0] == "help":
            print_command_help(command_path)
            sys.exit(0)
        for a in args:
            if a in ("--help", "-h"):
                print_command_help(command_path)
                sys.exit(0)
```

- [ ] **Step 3: Add `_check_help` as the first line of every existing command function**

Edit each of these functions in `k8s/sam/20_vikunja_tool_configmap.yaml` and insert the matching `_check_help` call as the first statement inside each function body (after the docstring):

```python
    def cmd_projects(args):
        """List all projects."""
        _check_help(args, "projects")
        # ... existing body ...
```

```python
    def cmd_project_create(args):
        """Create a new project."""
        _check_help(args, "project create")
        # ... existing body ...
```

```python
    def cmd_tasks(args):
        """List tasks in a project."""
        _check_help(args, "tasks")
        # ... existing body ...
```

```python
    def cmd_task_create(args):
        """Create a task in a project."""
        _check_help(args, "task create")
        # ... existing body ...
```

```python
    def cmd_task_update(args):
        """Update a task."""
        _check_help(args, "task update")
        # ... existing body ...
```

```python
    def cmd_task_assign(args):
        """Assign a user to a task."""
        _check_help(args, "task assign")
        # ... existing body ...
```

```python
    def cmd_task_comment(args):
        """Add a comment to a task."""
        _check_help(args, "task comment")
        # ... existing body ...
```

- [ ] **Step 4: Update `main()` to route `help <cmd>` / `vikunja help <cmd>`**

Replace the existing top-of-`main()` help clause:

```python
    def main():
        if len(sys.argv) < 2 or sys.argv[1] in ("help", "--help", "-h"):
            print(__doc__)
            sys.exit(0)
```

with:

```python
    def main():
        if len(sys.argv) < 2:
            print(__doc__)
            sys.exit(0)
        if sys.argv[1] in ("help", "--help", "-h"):
            # `vikunja help`              -> top-level
            # `vikunja help task create`  -> per-command
            if len(sys.argv) >= 3:
                print_command_help(" ".join(sys.argv[2:]))
            else:
                print(__doc__)
            sys.exit(0)
```

Leave the rest of `main()` unchanged (command dispatch is updated in later tasks as new commands are added).

- [ ] **Step 5: Syntax-check after edits**

```bash
python3 -c "
import yaml
data = yaml.safe_load(open('k8s/sam/20_vikunja_tool_configmap.yaml'))
open('/tmp/vikunja.py', 'w').write(data['data']['vikunja.py'])
"
python3 -m py_compile /tmp/vikunja.py && echo OK
```

Expected: `OK`. If you get a `SyntaxError`, the indentation inside the block scalar is wrong — re-open the YAML file and confirm every line of the inserted block starts with exactly 4 spaces of leading indent.

- [ ] **Step 6: Dry-run help output locally (no API calls needed)**

```bash
python3 /tmp/vikunja.py help
python3 /tmp/vikunja.py help task create
python3 /tmp/vikunja.py projects --help
```

Expected output for each:
1. `vikunja help` — the top-level docstring (unchanged).
2. `vikunja help task create` — the "Usage: vikunja task create..." block you added.
3. `vikunja projects --help` — the "Usage: vikunja projects" block (exits 0, does not try to reach the API).

- [ ] **Step 7: Commit**

```bash
git add k8s/sam/20_vikunja_tool_configmap.yaml
git commit -m "feat(k8s/sam): add per-command --help routing to vikunja CLI"
```

---

## Task 2: `task show` and `task delete` Subcommands

**Files:**
- Modify: `k8s/sam/20_vikunja_tool_configmap.yaml`

Add a detail view (so Sam can see a task's full description, labels, and assignees without having to scan JSON) and a delete command (so she can remove duplicates).

- [ ] **Step 1: Add `_print_task_detail`, `cmd_task_show`, and `cmd_task_delete` after the existing `cmd_task_comment` function**

Insert this block right after the end of `def cmd_task_comment(args):` and before `def main():`:

```python
    def _print_task_detail(task, comments):
        """Render a task dict as a multi-line detail view, including comments."""
        tid = task.get("id", "?")
        title = task.get("title", "Untitled")
        done = "DONE" if task.get("done") else "TODO"
        priority = task.get("priority", 0)
        pri_str = f"P{priority}" if priority else "(none)"
        due = task.get("due_date", "")
        due_str = due[:10] if due and due != "0001-01-01T00:00:00Z" else "(none)"
        labels = task.get("labels") or []
        label_str = ", ".join(l.get("title", "?") for l in labels) or "(none)"
        assignees = task.get("assignees") or []
        assign_str = ", ".join(f"@{a.get('username','?')}" for a in assignees) or "(none)"
        desc = task.get("description", "") or ""
        print(f"#{tid} [{done}] {title}")
        print(f"  priority:  {pri_str}")
        print(f"  due:       {due_str}")
        print(f"  labels:    {label_str}")
        print(f"  assignees: {assign_str}")
        if desc:
            print(f"  description:")
            for line in desc.splitlines():
                print(f"    {line}")
        if comments:
            print(f"  comments ({len(comments)}):")
            for c in comments:
                author = (c.get("author") or {}).get("username", "?")
                created = (c.get("created") or "")[:10]
                body = (c.get("comment") or "").strip()
                header = f"    - @{author}"
                if created:
                    header += f" ({created})"
                print(header)
                for line in body.splitlines() or [""]:
                    print(f"        {line}")
        else:
            print(f"  comments:  (none)")


    def cmd_task_show(args):
        """Show full details for a single task, including comments."""
        _check_help(args, "task show")
        if not args:
            print_command_help("task show")
            sys.exit(1)
        task_id = args[0]
        task = _api("GET", f"/tasks/{task_id}")
        # Comments are on a separate endpoint; an empty list is a valid response.
        comments = _api("GET", f"/tasks/{task_id}/comments") or []
        _print_task_detail(task, comments)


    def cmd_task_delete(args):
        """Delete a task permanently."""
        _check_help(args, "task delete")
        if not args:
            print_command_help("task delete")
            sys.exit(1)
        task_id = args[0]
        _api("DELETE", f"/tasks/{task_id}")
        print(f"Task #{task_id} deleted")
```

- [ ] **Step 2: Wire `task show` and `task delete` into `main()`**

Find the existing `elif cmd == "task" and rest:` block in `main()`:

```python
        elif cmd == "task" and rest:
            subcmd = rest[0]
            if subcmd == "create":
                cmd_task_create(rest[1:])
            elif subcmd == "update":
                cmd_task_update(rest[1:])
            elif subcmd == "assign":
                cmd_task_assign(rest[1:])
            elif subcmd == "comment":
                cmd_task_comment(rest[1:])
            else:
                print(f"Unknown task subcommand: {subcmd}", file=sys.stderr)
                sys.exit(1)
```

Replace with:

```python
        elif cmd == "task" and rest:
            subcmd = rest[0]
            if subcmd == "create":
                cmd_task_create(rest[1:])
            elif subcmd == "update":
                cmd_task_update(rest[1:])
            elif subcmd == "assign":
                cmd_task_assign(rest[1:])
            elif subcmd == "comment":
                cmd_task_comment(rest[1:])
            elif subcmd == "show":
                cmd_task_show(rest[1:])
            elif subcmd == "delete":
                cmd_task_delete(rest[1:])
            else:
                print(f"Unknown task subcommand: {subcmd}", file=sys.stderr)
                print(__doc__)
                sys.exit(1)
```

- [ ] **Step 3: Syntax-check**

```bash
python3 -c "
import yaml
data = yaml.safe_load(open('k8s/sam/20_vikunja_tool_configmap.yaml'))
open('/tmp/vikunja.py', 'w').write(data['data']['vikunja.py'])
"
python3 -m py_compile /tmp/vikunja.py && echo OK
```

Expected: `OK`.

- [ ] **Step 4: Dry-run help output for new commands**

```bash
python3 /tmp/vikunja.py task show --help
python3 /tmp/vikunja.py task delete --help
```

Expected: both print their usage blocks and exit 0.

- [ ] **Step 5: Commit**

```bash
git add k8s/sam/20_vikunja_tool_configmap.yaml
git commit -m "feat(k8s/sam): add vikunja task show and task delete subcommands"
```

---

## Task 3: `labels`, `label create`, `task label` (add/remove/list)

**Files:**
- Modify: `k8s/sam/20_vikunja_tool_configmap.yaml`

Add label management so Sam can create color-coded labels (her main priority signal) and attach/detach/list them on tasks by name.

- [ ] **Step 1: Add `urllib.parse` import**

Find the existing import block at the top of the embedded script:

```python
    import json
    import os
    import sys
    import urllib.request
    import urllib.error
```

Add `import urllib.parse` so it reads:

```python
    import json
    import os
    import sys
    import urllib.parse
    import urllib.request
    import urllib.error
```

- [ ] **Step 2: Add `_find_label_by_name`, `cmd_labels`, `cmd_label_create`, and `cmd_task_label` after `cmd_task_delete`**

Insert this block right after the end of `def cmd_task_delete(args):`:

```python
    def _find_label_by_name(name):
        """Return the label dict matching `name` exactly, or None."""
        q = urllib.parse.quote(name)
        results = _api("GET", f"/labels?s={q}")
        for l in results or []:
            if l.get("title") == name:
                return l
        return None


    def cmd_labels(args):
        """List all labels."""
        _check_help(args, "labels")
        flags, _ = _parse_args(args, {"search": "str"})
        path = "/labels"
        if "search" in flags:
            path += f"?s={urllib.parse.quote(flags['search'])}"
        labels = _api("GET", path)
        if not labels:
            print("No labels found.")
            return
        for l in labels:
            lid = l.get("id", "?")
            title = l.get("title", "Untitled")
            color = l.get("hex_color") or ""
            color_str = f" #{color}" if color else ""
            desc = l.get("description", "") or ""
            desc_str = f" — {desc[:60]}" if desc else ""
            print(f"  #{lid}: {title}{color_str}{desc_str}")


    def cmd_label_create(args):
        """Create a new label."""
        _check_help(args, "label create")
        flags, _ = _parse_args(args, {"title": "str", "color": "str", "description": "str"})
        title = flags.get("title")
        if not title:
            print_command_help("label create")
            sys.exit(1)
        data = {"title": title}
        if "color" in flags:
            data["hex_color"] = flags["color"].lstrip("#")
        if "description" in flags:
            data["description"] = flags["description"]
        result = _api("PUT", "/labels", data)
        print(f"Label #{result.get('id', '?')} created: {result.get('title', title)}")


    def cmd_task_label(args):
        """Manage labels on a task."""
        _check_help(args, "task label")
        flags, positional = _parse_args(args, {
            "add": "str",
            "remove": "str",
            "list": "bool",
            "create-missing": "bool",
            "color": "str",
        })
        if not positional:
            print_command_help("task label")
            sys.exit(1)
        task_id = positional[0]

        if flags.get("list"):
            task = _api("GET", f"/tasks/{task_id}")
            labels = task.get("labels") or []
            if not labels:
                print(f"Task #{task_id} has no labels.")
                return
            for l in labels:
                lid = l.get("id", "?")
                title = l.get("title", "Untitled")
                color = l.get("hex_color") or ""
                color_str = f" #{color}" if color else ""
                print(f"  #{lid}: {title}{color_str}")
            return

        if "add" in flags:
            name = flags["add"]
            label = _find_label_by_name(name)
            if not label:
                if flags.get("create-missing"):
                    create_data = {"title": name}
                    if "color" in flags:
                        create_data["hex_color"] = flags["color"].lstrip("#")
                    label = _api("PUT", "/labels", create_data)
                    print(f"  Created label #{label.get('id', '?')}: {name}")
                else:
                    print(f"ERROR: Label '{name}' not found. Use --create-missing to auto-create.",
                          file=sys.stderr)
                    sys.exit(1)
            _api("PUT", f"/tasks/{task_id}/labels", {"label_id": label["id"]})
            print(f"  Attached label '{name}' (#{label['id']}) to task #{task_id}")
            return

        if "remove" in flags:
            name = flags["remove"]
            task = _api("GET", f"/tasks/{task_id}")
            task_labels = task.get("labels") or []
            match = next((l for l in task_labels if l.get("title") == name), None)
            if not match:
                print(f"ERROR: Task #{task_id} does not have label '{name}'.", file=sys.stderr)
                sys.exit(1)
            _api("DELETE", f"/tasks/{task_id}/labels/{match['id']}")
            print(f"  Detached label '{name}' (#{match['id']}) from task #{task_id}")
            return

        print_command_help("task label")
        sys.exit(1)
```

- [ ] **Step 3: Wire `labels`, `label create`, and `task label` into `main()`**

Find the existing dispatcher block in `main()` that starts with `if cmd == "projects":` and replace the whole dispatcher (from `if cmd == "projects":` through the final `else:` that prints the unknown-command error) with:

```python
        if cmd == "projects":
            cmd_projects(rest)
        elif cmd == "project" and rest and rest[0] == "create":
            cmd_project_create(rest[1:])
        elif cmd == "tasks":
            cmd_tasks(rest)
        elif cmd == "labels":
            cmd_labels(rest)
        elif cmd == "label" and rest and rest[0] == "create":
            cmd_label_create(rest[1:])
        elif cmd == "task" and rest:
            subcmd = rest[0]
            if subcmd == "create":
                cmd_task_create(rest[1:])
            elif subcmd == "update":
                cmd_task_update(rest[1:])
            elif subcmd == "assign":
                cmd_task_assign(rest[1:])
            elif subcmd == "comment":
                cmd_task_comment(rest[1:])
            elif subcmd == "show":
                cmd_task_show(rest[1:])
            elif subcmd == "delete":
                cmd_task_delete(rest[1:])
            elif subcmd == "label":
                cmd_task_label(rest[1:])
            else:
                print(f"Unknown task subcommand: {subcmd}", file=sys.stderr)
                print(__doc__)
                sys.exit(1)
        else:
            print(f"Unknown command: {cmd}", file=sys.stderr)
            print(__doc__)
            sys.exit(1)
```

- [ ] **Step 4: Syntax-check**

```bash
python3 -c "
import yaml
data = yaml.safe_load(open('k8s/sam/20_vikunja_tool_configmap.yaml'))
open('/tmp/vikunja.py', 'w').write(data['data']['vikunja.py'])
"
python3 -m py_compile /tmp/vikunja.py && echo OK
```

Expected: `OK`.

- [ ] **Step 5: Dry-run help output**

```bash
python3 /tmp/vikunja.py labels --help
python3 /tmp/vikunja.py label create --help
python3 /tmp/vikunja.py task label --help
```

Expected: all three print their usage blocks.

- [ ] **Step 6: Commit**

```bash
git add k8s/sam/20_vikunja_tool_configmap.yaml
git commit -m "feat(k8s/sam): add vikunja labels, label create, and task label subcommands"
```

---

## Task 4: `tasks reorder` — Bulk Reorder Tasks in a Project

**Files:**
- Modify: `k8s/sam/20_vikunja_tool_configmap.yaml`

Add the feature Sam needed most today: a single command that rewrites the list-view ordering of a project's tasks in one call. The CLI accepts a comma-separated list of task IDs in the desired order, resolves the project's default list view, and POSTs to `/api/v1/tasks/{id}/position` for each task with a strictly increasing float position.

Vikunja stores per-view task positions in a `task_positions` table. The canonical write path is `POST /api/v1/tasks/{taskID}/position` with body `{"project_view_id": N, "position": float}`. Default spacing in the UI is 65536.0, so we use the same step to leave room for future fine-grained inserts without immediate re-spacing.

- [ ] **Step 1: Add `_get_list_view_id` helper and `cmd_tasks_reorder`**

Insert this block right after `cmd_task_label` (before `def main():`):

```python
    def _get_list_view_id(project_id):
        """Return the ID of the first 'list' view for a project, or the first view if none is list-kind."""
        views = _api("GET", f"/projects/{project_id}/views")
        if not views:
            print(f"ERROR: Project #{project_id} has no views.", file=sys.stderr)
            sys.exit(1)
        for v in views:
            if v.get("view_kind") == "list":
                return v["id"]
        return views[0]["id"]


    def cmd_tasks_reorder(args):
        """Resequence every task in a project so --order tasks come first, rest follow."""
        _check_help(args, "tasks reorder")
        flags, positional = _parse_args(args, {"order": "str"})
        if not positional or "order" not in flags:
            print_command_help("tasks reorder")
            sys.exit(1)
        project_id = positional[0]
        requested = [x.strip() for x in flags["order"].split(",") if x.strip()]
        if not requested:
            print("ERROR: --order must list at least one task ID", file=sys.stderr)
            sys.exit(1)

        # Reject duplicate IDs in --order — ambiguous intent and would crash
        # the "remaining tasks" computation below.
        seen = set()
        dupes = []
        for tid in requested:
            if tid in seen:
                dupes.append(tid)
            seen.add(tid)
        if dupes:
            print(f"ERROR: --order contains duplicate IDs: {','.join(dupes)}", file=sys.stderr)
            sys.exit(1)

        view_id = _get_list_view_id(project_id)
        current = _api("GET", f"/projects/{project_id}/views/{view_id}/tasks") or []
        # Current list-view ordering is the source of truth for "remaining tasks".
        current_ids = [str(t.get("id")) for t in current]
        current_set = set(current_ids)

        unknown = [tid for tid in requested if tid not in current_set]
        if unknown:
            print(
                f"ERROR: task ID(s) not in project #{project_id} (view #{view_id}): "
                f"{','.join(unknown)}",
                file=sys.stderr,
            )
            sys.exit(1)

        # Full deterministic sequence: explicit IDs first (in user order),
        # then every remaining ID in its existing display order. This makes
        # the operation a total resequence, not a partial rewrite — so the
        # final order cannot depend on whatever stale positions happened to
        # exist before the command ran.
        requested_set = set(requested)
        remaining = [tid for tid in current_ids if tid not in requested_set]
        final_order = requested + remaining

        print(
            f"Resequencing {len(final_order)} task(s) in project #{project_id} "
            f"(view #{view_id}): {len(requested)} explicit + {len(remaining)} trailing"
        )

        # Match Vikunja's native spacing so future fine-grained inserts can
        # slot between existing tasks without needing a full re-space.
        step = 65536.0
        for i, task_id in enumerate(final_order):
            position = step * (i + 1)
            marker = "*" if task_id in requested_set else " "
            _api("POST", f"/tasks/{task_id}/position", {
                "project_view_id": int(view_id),
                "position": position,
            })
            print(f"  {marker} #{task_id} -> position {position}")
        print(f"Done. Resequenced {len(final_order)} task(s) (* = from --order).")
```

- [ ] **Step 2: Refactor `cmd_tasks` to use `_get_list_view_id` (DRY)**

The existing `cmd_tasks` inlines its own view-lookup logic. Replace that inline lookup with a call to the new helper. Find this block inside `cmd_tasks`:

```python
        project_id = args[0]
        # Get the list view (first view) for this project
        views = _api("GET", f"/projects/{project_id}/views")
        if not views:
            print("No views found for this project.")
            return
        view_id = views[0].get("id")
        task_list = _api("GET", f"/projects/{project_id}/views/{view_id}/tasks")
```

and replace it with:

```python
        project_id = args[0]
        view_id = _get_list_view_id(project_id)
        task_list = _api("GET", f"/projects/{project_id}/views/{view_id}/tasks")
```

- [ ] **Step 3: Wire `tasks reorder` into `main()`**

Find the existing `elif cmd == "tasks":` line in the dispatcher:

```python
        elif cmd == "tasks":
            cmd_tasks(rest)
```

Replace with:

```python
        elif cmd == "tasks":
            if rest and rest[0] == "reorder":
                cmd_tasks_reorder(rest[1:])
            else:
                cmd_tasks(rest)
```

- [ ] **Step 4: Syntax-check**

```bash
python3 -c "
import yaml
data = yaml.safe_load(open('k8s/sam/20_vikunja_tool_configmap.yaml'))
open('/tmp/vikunja.py', 'w').write(data['data']['vikunja.py'])
"
python3 -m py_compile /tmp/vikunja.py && echo OK
```

Expected: `OK`.

- [ ] **Step 5: Dry-run help**

```bash
python3 /tmp/vikunja.py tasks reorder --help
```

Expected: prints the "Usage: vikunja tasks reorder..." block.

- [ ] **Step 6: Commit**

```bash
git add k8s/sam/20_vikunja_tool_configmap.yaml
git commit -m "feat(k8s/sam): add vikunja tasks reorder subcommand for bulk task ordering"
```

---

## Task 5: `tasks` `--sort` and `--format` Flags

**Files:**
- Modify: `k8s/sam/20_vikunja_tool_configmap.yaml`

Let Sam view the same task list sorted by priority or due date (without mutating the stored order) and get a machine-readable JSON dump she can pipe into the shell tool for duplicate detection.

- [ ] **Step 1: Replace `cmd_tasks` with the extended version**

Find the current `cmd_tasks` function (after Task 4's edits it reads the view via `_get_list_view_id`). Replace the entire function body with:

```python
    def cmd_tasks(args):
        """List tasks in a project."""
        _check_help(args, "tasks")
        flags, positional = _parse_args(args, {"sort": "str", "format": "str"})
        if not positional:
            print_command_help("tasks")
            sys.exit(1)
        project_id = positional[0]
        view_id = _get_list_view_id(project_id)
        task_list = _api("GET", f"/projects/{project_id}/views/{view_id}/tasks")
        if not task_list:
            print(f"No tasks in project #{project_id}.")
            return

        sort_by = flags.get("sort", "position")
        if sort_by == "priority":
            # Highest priority first; tasks with no priority (0) sink to the bottom.
            task_list.sort(key=lambda t: -(t.get("priority") or 0))
        elif sort_by == "due":
            # Earliest due first; tasks with no due date sink to the bottom.
            def _due_key(t):
                d = t.get("due_date") or ""
                if not d or d == "0001-01-01T00:00:00Z":
                    return "9999-99-99"
                return d
            task_list.sort(key=_due_key)
        # "position" is the native order returned by the API; no-op.

        if flags.get("format") == "json":
            print(json.dumps(task_list, indent=2))
            return

        for t in task_list:
            _print_task_line(t)
```

- [ ] **Step 2: Extract `_print_task_line` helper (DRY)**

Insert this new helper right before `cmd_tasks`:

```python
    def _print_task_line(t):
        """Render one task as a single-line list entry."""
        tid = t.get("id", "?")
        title = t.get("title", "Untitled")
        done = "DONE" if t.get("done") else "TODO"
        due = t.get("due_date", "")
        due_str = f" (due: {due[:10]})" if due and due != "0001-01-01T00:00:00Z" else ""
        priority = t.get("priority", 0)
        pri_str = f" [P{priority}]" if priority else ""
        labels = t.get("labels") or []
        label_str = (" {" + ",".join(l.get("title", "?") for l in labels) + "}") if labels else ""
        assignees = t.get("assignees") or []
        assign_str = f" @{','.join(a.get('username','?') for a in assignees)}" if assignees else ""
        print(f"  #{tid} [{done}]{pri_str} {title}{due_str}{label_str}{assign_str}")
```

- [ ] **Step 3: Syntax-check**

```bash
python3 -c "
import yaml
data = yaml.safe_load(open('k8s/sam/20_vikunja_tool_configmap.yaml'))
open('/tmp/vikunja.py', 'w').write(data['data']['vikunja.py'])
"
python3 -m py_compile /tmp/vikunja.py && echo OK
```

Expected: `OK`.

- [ ] **Step 4: Commit**

```bash
git add k8s/sam/20_vikunja_tool_configmap.yaml
git commit -m "feat(k8s/sam): add --sort and --format json flags to vikunja tasks"
```

---

## Task 6: Refresh the Top-Level `__doc__` String

**Files:**
- Modify: `k8s/sam/20_vikunja_tool_configmap.yaml`

The top-level docstring (what `vikunja help` prints) still lists only the original command set. Update it to include every new command so Sam can discover them from a single help call.

- [ ] **Step 1: Replace the existing top-level docstring**

Find the `"""Vikunja CLI for Sam (ZeroClaw Agent).` block at the top of the embedded script and replace it with:

```python
    """Vikunja CLI for Sam (ZeroClaw Agent).

    Manages projects, tasks, and labels on the Vikunja instance at
    todolist.coffee-anon.com for project status tracking and coordination.

    Commands:
      vikunja projects                              List all projects
      vikunja project create --title "..."          Create a project

      vikunja tasks <project-id> [--sort FIELD] [--format FORMAT]
                                                    List tasks in a project
      vikunja tasks reorder <project-id> --order "id1,id2,id3"
                                                    Resequence a project: listed IDs move to the
                                                    top in order, unlisted tasks follow in their
                                                    current order

      vikunja task create <project-id> --title "..." [--description ...] [--due YYYY-MM-DD] [--priority 1-5] [--assignee user]
      vikunja task update <task-id> [--done] [--title ...] [--description ...] [--due ...] [--priority 1-5] [--assignee user]
      vikunja task show   <task-id>                 Full task detail (description, labels, assignees, comments)
      vikunja task delete <task-id>                 Delete a task (use for duplicates)
      vikunja task assign <task-id> --user <username>
      vikunja task comment <task-id> --body "..."

      vikunja task label <task-id> --list
      vikunja task label <task-id> --add "name" [--create-missing] [--color ff0000]
      vikunja task label <task-id> --remove "name"

      vikunja labels [--search "..."]               List all labels
      vikunja label create --title "..." [--color "ff0000"] [--description "..."]

      vikunja help                                  Top-level help
      vikunja help <command>                        Per-command help
      vikunja <command> --help                      Same (works on any subcommand)

    Environment:
      VIKUNJA_API_TOKEN  — JWT token for the sam user (required)
      VIKUNJA_BASE_URL   — Vikunja URL (default: http://vikunja.todolist.svc.cluster.local:3456)
    """
```

- [ ] **Step 2: Syntax-check**

```bash
python3 -c "
import yaml
data = yaml.safe_load(open('k8s/sam/20_vikunja_tool_configmap.yaml'))
open('/tmp/vikunja.py', 'w').write(data['data']['vikunja.py'])
"
python3 -m py_compile /tmp/vikunja.py && echo OK
```

Expected: `OK`.

- [ ] **Step 3: Dry-run top-level help**

```bash
python3 /tmp/vikunja.py help
```

Expected: the new docstring prints and lists every command including `tasks reorder`, `task show`, `task delete`, `task label`, `labels`, `label create`.

- [ ] **Step 4: Commit**

```bash
git add k8s/sam/20_vikunja_tool_configmap.yaml
git commit -m "docs(k8s/sam): refresh vikunja CLI top-level help with new commands"
```

---

## Task 7: Update the `vikunja-project-manager` Skill

**Files:**
- Modify: `k8s/sam/13_zeroclaw_skills_configmap.yaml`

Teach Sam when to use each new subcommand and give her concrete workflows for "remove duplicates" and "prioritize the list" — the exact tasks she struggled with today.

- [ ] **Step 1: Locate the existing `vikunja-project-manager.md:` key**

Open `k8s/sam/13_zeroclaw_skills_configmap.yaml` and find the `vikunja-project-manager.md: |` key. Everything between that key and the next skill entry (or EOF) is the skill body.

- [ ] **Step 2: Replace the entire skill body with the updated version**

Replace the contents of `vikunja-project-manager.md` with:

```yaml
  vikunja-project-manager.md: |
    ---
    name: vikunja-project-manager
    version: 2.0.0
    description: Manage project status, tasks, priorities, labels, and ordering via the vikunja CLI. Use whenever Dan asks about project status, task tracking, TODO lists, prioritizing a list, removing duplicates, or wants to communicate progress on work items.
    ---

    # Vikunja Project Manager

    You have a `vikunja` CLI tool that manages projects, tasks, labels, and
    ordering on the team's Vikunja instance (todolist.coffee-anon.com). Use it
    to track work status, create actionable task lists, and report progress.

    ## When to use this

    - Dan asks about project status or what's being worked on
    - Dan asks you to prioritize, reorder, or clean up a list
    - Dan asks you to remove duplicates from a project
    - You complete work and want to record it
    - You need to check what's outstanding before starting work

    ## Discovering usage

    Every command supports `--help`. When you are unsure, run it:

    ```
    vikunja help                     # Top-level command list
    vikunja help task create         # Detailed help for a specific command
    vikunja task label --help        # Same, via --help on the subcommand
    ```

    ## Command reference

    ```
    vikunja projects
    vikunja project create --title "..."

    vikunja tasks <project-id> [--sort position|priority|due] [--format text|json]
    vikunja tasks reorder <project-id> --order "id1,id2,id3"

    vikunja task create <project-id> --title "..." [--description "..."] [--due "YYYY-MM-DD"] [--priority 1-5] [--assignee user]
    vikunja task update <task-id>   [--done] [--title "..."] [--description "..."] [--due "..."] [--priority 1-5] [--assignee user]
    vikunja task show   <task-id>
    vikunja task delete <task-id>
    vikunja task assign <task-id> --user <username>
    vikunja task comment <task-id> --body "..."

    vikunja task label <task-id> --list
    vikunja task label <task-id> --add "name" [--create-missing] [--color ff0000]
    vikunja task label <task-id> --remove "name"

    vikunja labels [--search "..."]
    vikunja label create --title "..." [--color "ff0000"] [--description "..."]
    ```

    ## Two ways to signal priority

    Vikunja has two independent priority signals. Use them together:

    1. **Native priority integer (1-5)** — stored on the task itself, used by
       `--sort priority`. Set it with `vikunja task update <id> --priority 5`.
       This is the field the UI sorts and color-codes by default.
    2. **Labels** — free-form colored tags. Useful for named priorities
       ("urgent", "blocked", "waiting") that should be visible at a glance
       in the list view. Attach with `vikunja task label <id> --add "urgent"`.

    When Dan asks you to "prioritize" a list, set both the integer priority
    AND an appropriate label so the signal is visible in every view.

    ## Workflow: prioritize an existing project

    1. `vikunja tasks <project-id>` — read the current list.
    2. Decide the priority order in your head.
    3. For each task, `vikunja task update <id> --priority N` (1=lowest, 5=highest).
    4. Optionally attach labels: `vikunja task label <id> --add "urgent" --create-missing --color ff0000`.
    5. Rewrite the display order in one call:
       `vikunja tasks reorder <project-id> --order "id-of-top,id-of-next,..."`.
    6. Verify with `vikunja tasks <project-id> --sort priority`.

    ## Workflow: remove duplicates

    1. `vikunja tasks <project-id> --format json` — get machine-readable task data.
    2. Scan the JSON for tasks with the same or near-identical `title`.
    3. For each duplicate, pick the one to keep (usually the oldest by `id`
       or the one with more context — comments, labels, assignees).
    4. For each duplicate to remove, `vikunja task delete <id>`.
    5. Verify with `vikunja tasks <project-id>`.

    ## Workflow: starting a new initiative

    1. `vikunja project create --title "Initiative Name"`
    2. Create tasks in priority order:
       `vikunja task create <id> --title "..." --priority 4`
    3. Reorder if you created them out of sequence:
       `vikunja tasks reorder <id> --order "..."`
    4. Report the project ID and task list to Dan.

    ## Tips

    - `--priority` is 1 (lowest) through 5 (highest).
    - `--sort priority` is read-only — it changes the display of `vikunja tasks`
      but does not change the stored order. Use `tasks reorder` to persist.
    - `task show` is your detail view — use it whenever a list line isn't enough.
    - `--format json` is the right tool for programmatic reasoning; the default
      text format is for quick human-facing status reports.
    - Keep task titles short and actionable ("Deploy vikunja postgres", not
      "We need to set up the database").
```

- [ ] **Step 3: Validate YAML**

```bash
python3 -c "import yaml; yaml.safe_load(open('k8s/sam/13_zeroclaw_skills_configmap.yaml')); print('OK')"
```

Expected: `OK`.

- [ ] **Step 4: Commit**

```bash
git add k8s/sam/13_zeroclaw_skills_configmap.yaml
git commit -m "feat(k8s/sam): teach vikunja-project-manager skill reorder, label, and dedupe workflows"
```

---

## Task 8: Deploy and Validate End-to-End

**Files:**
- Modify: (none — applying existing committed ConfigMaps)

This task deploys both updated ConfigMaps, bounces Sam's pod, and runs every new subcommand against the real Vikunja instance so API contract mismatches are caught before handing off to Sam.

**Rerun safety**: every name created during validation is timestamped (`CLI-validation-<epoch>`, `urgent-test-<epoch>`, etc.) so repeated runs of this task cannot collide on global-by-name resources (Vikunja labels are global, not per-project). Final cleanup deletes every scratch resource this task creates — labels, tasks, and the scratch project itself — via the Vikunja API (label and project delete don't have CLI subcommands, so the cleanup uses `python3 -c` + `_api`).

- [ ] **Step 1: Apply both ConfigMaps**

```bash
kubectl apply -f k8s/sam/20_vikunja_tool_configmap.yaml
kubectl apply -f k8s/sam/13_zeroclaw_skills_configmap.yaml
```

Expected: both print `configmap/... configured`.

- [ ] **Step 2: Restart Sam's pod to pick up the new ConfigMap content**

The `vikunja` script is mounted as a subPath ConfigMap volume at `/usr/local/bin/vikunja` (see `k8s/sam/04_zeroclaw_sandbox.yaml:181-184`). **subPath volume mounts do not receive hot updates** from kubelet — the file inside the container is resolved once at container start. A pod restart is the only way to pick up new ConfigMap content for a subPath mount.

```bash
kubectl delete pod -n ai-agents -l app=zeroclaw
kubectl wait --for=condition=Ready pod -n ai-agents -l app=zeroclaw --timeout=180s
```

Expected: a new pod comes up and becomes Ready within 180s (the warm-up path waits on the agent-browser daemon for up to ~60s, so keep the timeout generous).

- [ ] **Step 3: Verify top-level help shows every new command**

```bash
kubectl exec -n ai-agents -c zeroclaw $(kubectl get pod -n ai-agents -l app=zeroclaw -o name | head -1) -- vikunja help
```

Expected: the top-level docstring lists `tasks reorder`, `task show`, `task delete`, `task label`, `labels`, and `label create`.

- [ ] **Step 4: Verify per-command help (three routing styles)**

```bash
POD=$(kubectl get pod -n ai-agents -l app=zeroclaw -o name | head -1)
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja task create --help
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja help tasks reorder
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja task label help
```

Expected: each prints the corresponding `Usage:` block and exits 0 without touching the API. (The three forms — `--help`, `help <cmd>`, and `<cmd> help` — all work.)

- [ ] **Step 5: Allocate a timestamped suffix and a scratch project**

```bash
POD=$(kubectl get pod -n ai-agents -l app=zeroclaw -o name | head -1)
STAMP=$(date +%s)
SCRATCH_NAME="CLI-validation-$STAMP"
LABEL_NAME="urgent-test-$STAMP"
echo "STAMP=$STAMP SCRATCH_NAME=$SCRATCH_NAME LABEL_NAME=$LABEL_NAME"

kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja project create --title "$SCRATCH_NAME"
# Read the project ID from the output ("Project #N created: ...") and record it:
SCRATCH_PID=<id-from-output>
```

Record `$POD`, `$STAMP`, `$SCRATCH_PID`, `$SCRATCH_NAME`, and `$LABEL_NAME` — the cleanup step at the end uses them.

- [ ] **Step 6: Create four tasks in the scratch project (out of priority order)**

Using four tasks (not three) gives us a partial-reorder case to test: reorder two of them and confirm the other two end up sequenced after.

```bash
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja task create $SCRATCH_PID --title "task-c (should end up last of explicit block)" --priority 1
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja task create $SCRATCH_PID --title "task-a (should end up first)" --priority 5
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja task create $SCRATCH_PID --title "task-b (should end up middle of explicit block)" --priority 3
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja task create $SCRATCH_PID --title "task-d (unlisted — should trail)" --priority 2
# Record the four task IDs in creation order (c, a, b, d).
TASK_C=<id> ; TASK_A=<id> ; TASK_B=<id> ; TASK_D=<id>
```

- [ ] **Step 7: Verify `tasks` default order is creation order**

```bash
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja tasks $SCRATCH_PID
```

Expected: four tasks, currently in creation order (c, a, b, d).

- [ ] **Step 8: Verify `tasks --sort priority` reorders the display but doesn't persist**

```bash
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja tasks $SCRATCH_PID --sort priority
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja tasks $SCRATCH_PID
```

Expected: the sorted view shows a ([P5]), b ([P3]), d ([P2]), c ([P1]); the unsorted view is still creation order (c, a, b, d). This confirms `--sort` is display-only.

- [ ] **Step 9: Verify `tasks reorder` — partial reorder resequences the full view**

Reorder only task_a and task_b explicitly. Task_c and task_d are omitted — they should end up after the explicit block in their current display order (c, then d).

```bash
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja tasks reorder $SCRATCH_PID --order "$TASK_A,$TASK_B"
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja tasks $SCRATCH_PID --format json \
  | TASK_A=$TASK_A TASK_B=$TASK_B TASK_C=$TASK_C TASK_D=$TASK_D python3 -c "
import json, sys, os
ids = [str(t['id']) for t in json.load(sys.stdin)]
expected = [os.environ['TASK_A'], os.environ['TASK_B'], os.environ['TASK_C'], os.environ['TASK_D']]
print('Got:     ', ids)
print('Expected:', expected)
assert ids == expected, f'order mismatch: {ids} != {expected}'
print('OK — partial reorder placed unlisted tasks after the explicit block in display order')
"
```

The env-var assignments come *before* `python3` — in bash, `VAR=val cmd` syntax only exports the variable to `cmd` when the assignments precede the command. Trailing tokens (as in `python3 -c "..." TASK_A=1`) become positional argv, not environment, and `os.environ['TASK_A']` would then raise `KeyError`.

Expected: the Python assertion prints `OK`. The reorder output also prints `* #<id> -> position ...` for the explicit block and `  #<id> -> position ...` (no star) for the trailing block, with strictly increasing positions.

If this step fails with `HTTP 404` or `HTTP 405` from `/tasks/{id}/position`, the endpoint contract has changed. Check the Vikunja server version (`curl http://vikunja.todolist.svc.cluster.local:3456/api/v1/info`) and re-query context7 (`/go-vikunja/vikunja`, "task position endpoint") for the current shape. The documented fallback is the bucket-move endpoint `POST /projects/{pid}/views/{vid}/buckets/{bid}/tasks` with `{task_id, position}`; adapt `_get_list_view_id` to also resolve the default bucket for the list view and POST there instead.

- [ ] **Step 10: Verify `tasks reorder` rejects unknown and duplicate IDs**

```bash
# Unknown ID — should exit non-zero with "not in project" error.
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja tasks reorder $SCRATCH_PID --order "$TASK_A,999999" && echo UNEXPECTED || echo OK_UNKNOWN

# Duplicate ID — should exit non-zero with "duplicate IDs" error.
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja tasks reorder $SCRATCH_PID --order "$TASK_A,$TASK_A" && echo UNEXPECTED || echo OK_DUP
```

Expected: both commands print `OK_UNKNOWN` / `OK_DUP` (they exited non-zero as intended, and the `||` branch fired).

- [ ] **Step 11: Verify `task show` renders description, labels, assignees, and comments**

```bash
# Seed a description and a comment so we can verify the detail view covers both.
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja task update $TASK_A --description "Line one of the description.
Line two of the description."
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja task comment $TASK_A --body "First comment from the CLI validation run."
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja task comment $TASK_A --body "Second comment from the CLI validation run."

kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja task show $TASK_A
```

Expected: the output shows `priority: P5`, the two description lines, and a `comments (2):` block with both comment bodies prefixed by `@sam` and a date. If the comments block is absent or shows `(none)`, either `_print_task_detail` wasn't updated (Task 2) or the `/tasks/{id}/comments` endpoint returned an unexpected shape — inspect with `curl` against the API directly.

- [ ] **Step 12: Verify `label create` and `task label --add --create-missing`**

```bash
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja label create --title "$LABEL_NAME" --color "ff0000" --description "CLI validation label $STAMP"
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja labels --search "$LABEL_NAME"
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja task label $TASK_A --add "$LABEL_NAME"
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja task label $TASK_A --list
```

Expected: label is created, search finds it, it attaches to task-a, and `--list` prints `#<id>: $LABEL_NAME #ff0000`.

- [ ] **Step 13: Verify `task label --remove`**

```bash
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja task label $TASK_A --remove "$LABEL_NAME"
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja task label $TASK_A --list
```

Expected: removal succeeds and the second `--list` prints `Task #... has no labels.`

- [ ] **Step 14: Verify `task delete` on a duplicate**

```bash
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja task create $SCRATCH_PID --title "duplicate-of-task-a" --priority 5
# Record the new task ID as TASK_DUP.
TASK_DUP=<id>
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja task delete $TASK_DUP
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja tasks $SCRATCH_PID --format json \
  | TASK_DUP=$TASK_DUP python3 -c "
import json, sys, os
ids = [t['id'] for t in json.load(sys.stdin)]
dup = int(os.environ['TASK_DUP'])
print('Tasks remaining:', ids)
assert dup not in ids, f'delete failed — duplicate {dup} still present'
print('OK')
"
```

Same pattern: `TASK_DUP=$TASK_DUP` precedes `python3`, so the assignment applies to the Python process's environment.

Expected: duplicate is gone, `OK` prints.

- [ ] **Step 15: Full cleanup — delete all scratch tasks, the label, and the scratch project**

Vikunja has no CLI subcommand for label or project deletion (by design — YAGNI for Sam's normal workflow), so cleanup uses a one-shot `python3 -c` snippet that loads the installed script and calls its internal helpers. This runs *inside the zeroclaw container* so it picks up `VIKUNJA_API_TOKEN` and `VIKUNJA_BASE_URL` from the same env Sam uses.

**Why `runpy.run_path` and not `importlib.util.spec_from_file_location`:** the installed script lives at `/usr/local/bin/vikunja` with no `.py` suffix. `spec_from_file_location` returns `None` for files with no recognized Python suffix (it derives the loader from the extension), so a naive import-by-path fails with `AttributeError: 'NoneType' object has no attribute 'loader'`. `runpy.run_path` handles suffixless files directly and — crucially — we pass `run_name='vikunja_tool'` so the `if __name__ == "__main__"` guard at the bottom of the script stays False and `main()` is not invoked. The top-level helper definitions (`_api`, `_find_label_by_name`, …) and the `TOKEN`/`BASE_URL` env reads all run once, and `run_path` returns the resulting globals dict so we can call them.

```bash
# Delete the four scratch tasks (order doesn't matter; ignore failures).
for t in $TASK_A $TASK_B $TASK_C $TASK_D; do
  kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja task delete $t || true
done

# Delete the scratch label and scratch project via direct API calls.
kubectl exec -n ai-agents -c zeroclaw $POD -- env LABEL_NAME="$LABEL_NAME" SCRATCH_PID="$SCRATCH_PID" python3 -c '
import os
import runpy

ns = runpy.run_path("/usr/local/bin/vikunja", run_name="vikunja_tool")
_api = ns["_api"]
_find_label_by_name = ns["_find_label_by_name"]

label_name = os.environ["LABEL_NAME"]
scratch_pid = os.environ["SCRATCH_PID"]

label = _find_label_by_name(label_name)
if label:
    _api("DELETE", f"/labels/{label[\"id\"]}")
    print(f"Deleted label #{label[\"id\"]} ({label_name})")
else:
    print(f"Label {label_name} not found — nothing to delete")

_api("DELETE", f"/projects/{scratch_pid}")
print(f"Deleted project #{scratch_pid}")
'
```

Note the outer single-quotes wrapping the Python body. That keeps the caller shell from substituting or escaping anything inside the Python source — `$LABEL_NAME` / `$SCRATCH_PID` reach the Python process via the `env` prefix (which runs inside the pod), not via caller-shell substitution. The inner `label["id"]` dict access uses double quotes escaped as `\"` because the enclosing Python string uses double quotes; those backslashes are literal to the single-quoted shell wrapper and are interpreted by Python.

Expected: the script prints `Deleted label #N (...)` and `Deleted project #<SCRATCH_PID>`. If the label delete endpoint returns 404, the label was already removed (idempotent). If the project delete endpoint returns 404, the project was already removed.

Verify nothing is left:

```bash
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja labels --search "$LABEL_NAME"
kubectl exec -n ai-agents -c zeroclaw $POD -- vikunja projects | grep "$SCRATCH_NAME" && echo UNEXPECTED || echo OK_CLEAN
```

Expected: labels search returns `No labels found.` and `projects | grep` prints `OK_CLEAN` (the project is gone so `grep` exits non-zero and the `||` branch fires).

- [ ] **Step 16: Wiki checkpoint**

Per `CLAUDE.md` section 1.1, this work produces operational knowledge worth ingesting:

- New Sam capabilities (reorder, label, delete, show, per-command help) → `wiki/services/zeroclaw.md` Custom Fork Changes
- Vikunja API quirks if any (e.g. if the `/tasks/{id}/position` endpoint didn't work as expected and we fell back to the bucket endpoint) → `wiki/services/zeroclaw.md` operational notes

Run `/wiki-ingest` against this session's findings before declaring the task done.

---

## Summary

| Task | What | Files | Risk |
|------|------|-------|------|
| 1 | Per-command `--help` routing | `k8s/sam/20_vikunja_tool_configmap.yaml` | Low |
| 2 | `task show`, `task delete` | `k8s/sam/20_vikunja_tool_configmap.yaml` | Low |
| 3 | `labels`, `label create`, `task label` add/remove/list | `k8s/sam/20_vikunja_tool_configmap.yaml` | Low |
| 4 | `tasks reorder` bulk reorder via `/tasks/{id}/position` | `k8s/sam/20_vikunja_tool_configmap.yaml` | Medium — endpoint contract may vary by Vikunja version |
| 5 | `--sort` and `--format` flags on `tasks` | `k8s/sam/20_vikunja_tool_configmap.yaml` | Low |
| 6 | Refresh top-level `__doc__` | `k8s/sam/20_vikunja_tool_configmap.yaml` | Low |
| 7 | Update `vikunja-project-manager` skill | `k8s/sam/13_zeroclaw_skills_configmap.yaml` | Low |
| 8 | Deploy + E2E validate | (apply only) | Medium — depends on live Vikunja |

**Dependencies:** Tasks 1 → 2 → 3 → 4 → 5 → 6 are sequential edits to the same file (later tasks reference helpers and dispatcher state from earlier ones). Task 7 is independent and can be done any time after Task 6. Task 8 must run last and applies both ConfigMaps.

**Rollback:** Each task is a separate commit. Revert any single commit with `git revert <sha>` — because the edits are additive (new functions + new dispatcher branches), reverting one task leaves the tool working for all other commands. ConfigMap changes take effect on next pod restart. If Task 4's `/tasks/{id}/position` endpoint turns out to be wrong for our Vikunja version, the fallback is documented inline in Task 8 Step 9.
