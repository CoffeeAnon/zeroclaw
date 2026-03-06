# TOOLS.md - Sam's Toolkit

## Serena: Code Intelligence + Persistent Memory

Serena is an MCP server providing semantic code intelligence and persistent cross-session memory. Use it to read and navigate codebases intelligently, and to store anything worth remembering between sessions.

**Command:** `serena-mcp` (available in PATH via `~/.local/bin/serena-mcp`)
**Server:** `http://localhost:9121/mcp`

### Session Start

```bash
serena-mcp init    # Start session + activate home project
```

### Memories (persist across sessions)

```bash
serena-mcp call write_memory '{"memory_file_name":"context","content":"..."}'
serena-mcp call read_memory '{"memory_file_name":"context"}'
serena-mcp call list_memories '{}'
```

Good memory file names: `context`, `preferences`, `open-threads`, `projects`

### Switching Projects for Code Analysis

```bash
serena-mcp project pm-agents          # analyse pm-agents codebase
serena-mcp project sam                # back to home project (set up on first use)
```

### File & Navigation Tools

All paths are **relative to the active project root**. Use `"."` for the project root.

```bash
# List directory
serena-mcp call list_dir '{"relative_path":".","recursive":false}'

# Read a file
serena-mcp call read_file '{"relative_path":"SOUL.md"}'

# Find files by name/glob
serena-mcp call find_file '{"file_mask":"*.md","relative_path":"."}'

# Search for a pattern (regex)
serena-mcp call search_for_pattern '{"substring_pattern":"keyword"}'

# Create/overwrite a file
serena-mcp call create_text_file '{"relative_path":"notes.md","content":"# Notes\n..."}'
```

### Symbol Tools (for code analysis)

```bash
serena-mcp call get_symbols_overview '{"relative_path":"src/main.py"}'
serena-mcp call find_symbol '{"name_path_pattern":"MyClass","include_body":true}'
serena-mcp call find_referencing_symbols '{"name_path":"MyClass","relative_path":"src/main.py"}'
```

| Category | Tools |
|----------|-------|
| **Memory** | `write_memory`, `read_memory`, `list_memories`, `edit_memory`, `delete_memory` |
| **Symbols** | `find_symbol`, `find_referencing_symbols`, `get_symbols_overview` |
| **Editing** | `replace_content`, `replace_symbol_body`, `insert_after_symbol` |
| **Navigation** | `activate_project`, `get_current_config`, `list_dir`, `read_file` |

---

## Context7: Up-to-Date Library Documentation

Use Context7 to fetch current documentation and code examples for any library or framework. It pulls live docs rather than relying on training data — valuable any time you're writing against an API or need to verify current behaviour.

**Two-step pattern:**

**Step 1 — Resolve the library ID:**
- Tool: `context7__resolve-library-id`
- Params: `libraryName` (e.g., `"react"`), `query` (what you're trying to accomplish)
- Returns a list of matching libraries with IDs, descriptions, and quality scores — pick the best match

**Step 2 — Query the docs:**
- Tool: `context7__query-docs`
- Params: `libraryId` (from step 1, format `/org/project`), `query` (specific question or task)
- Returns relevant documentation sections and code examples

**When to use:**
- Writing code against a library you want to verify is current
- Checking whether an API or behaviour changed in a recent version
- Finding working examples for something non-obvious

---

## Playwright: Browser Automation

Use Playwright when a task requires a real browser — JS-rendered content, authenticated sessions, form interaction, screenshots, or verifying how something actually looks.

**Core operations:**

| Tool | Purpose |
|------|---------|
| `playwright__navigate` | Go to a URL |
| `playwright__screenshot` | Capture the current page |
| `playwright__click` | Click an element (CSS selector or visible text) |
| `playwright__fill` | Type into an input field |
| `playwright__select_option` | Select from a dropdown |
| `playwright__snapshot` | Get the accessibility tree (lighter than a screenshot for content extraction) |
| `playwright__evaluate` | Run arbitrary JavaScript on the page |
| `playwright__wait_for_selector` | Wait for an element to appear |

**When to use:**
- Research requiring navigation of a web page with dynamic content
- Verifying that a frontend change renders correctly
- Any workflow where an HTTP request alone won't work (login walls, JS-heavy apps)

**Practical notes:**
- Prefer `snapshot` over `screenshot` when you just need page content — it's faster and returns structured text
- Use `evaluate` for extracting data that isn't easily targeted by selectors
- For multi-step flows (login → navigate → action), chain operations in sequence and check state after each step

---

## Google Workspace (via gog-agent)

For email and calendar access:

```bash
# Email
gog-agent gmail search "query terms"
gog-agent gmail get <message-id>

# Calendar
gog-agent calendar events --days 7
gog-agent calendar events --from today --to "next friday"
```

**Syntax note:** `gmail search` / `gmail get` (not `list`). `calendar events` (not `calendar list`).

---

## Key Principles

1. **Answer first, explain second** — lead with the useful thing
2. **Track loose ends** — things mentioned but unresolved go in Serena memory (`open-threads`)
3. **Honest about uncertainty** — "I don't know, I can find out" is always acceptable
4. **Check before acting on consequential things** — confirm intent before actions with side effects
5. **Keep memory current** — update notes when context changes, especially decisions made

---

## Active Projects

*(Update as Dan's active work changes)*

---

## Skills Available

| Skill | Location | Purpose |
|-------|----------|---------|
| `k8s-health-check` | `~/.zeroclaw/shared/skills/k8s-health-check/` | Kubernetes cluster assessment |
| `pm-doc-builder` | Shared skills | PM documentation generation |
| `frontend-design-review` | Shared skills | Design and accessibility review |
