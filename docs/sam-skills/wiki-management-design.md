# `wiki-management` skill — design notes

Rationale, templates, and extended failure modes for Sam's wiki-management
skill. Moved out of the `SKILL.md` body so per-turn token cost stays low;
this doc is for humans maintaining the skill, not for Sam's runtime.

Canonical skill body: `k8s/sam/25_wiki_management_skill_configmap.yaml`.

## Why absolute paths are mandatory

The wiki lives at `/data/workspace/wiki/`. Sam's cwd is
`/data/.zeroclaw/workspace/` — a different directory on the same PVC.
Relative paths like `wiki/index.md` or `entities/dan-jacobsen.md`
silently resolve into cwd and miss the real wiki tree — "file not
found" errors for pages that obviously exist. `workspace_only = false`
in the security policy makes absolute paths reachable; relative paths
still join against cwd.

Runtime rule in the skill body is one sentence. This longer version is
for anyone debugging a "not found" on a page they can see in `ls`.

## Why "don't re-read in the same turn" matters

If Sam reads a page with `file_read` or `shell: cat` and then reads it
again in the same turn, every subsequent request carries the page's
content twice. A 10 KB page re-read three times adds ~7500 tokens to
each remaining turn's request body. On `litellm-sam-cron` (Tier 0.5)
with Qwen 3.6 35B-A3B inference, prompts this bloated reliably hit the
300s HTTP timeout in `reliability.provider_request_timeout_secs` and
cascade into `error sending request for url` retries.

The 2026-04-13 meeting-summary cascade was the first time we saw this
pattern in production. Fix landed in v1.5.13 (300s timeout) and in
skill bodies everywhere ("don't re-read"). Runtime rule is one
sentence; root story is here.

If Sam genuinely needs to re-examine a page after a `file_edit` (e.g.,
to confirm a section landed where expected), use `file_read` with
`offset` and `limit` to pull only the relevant slice.

## Bootstrap — full content (reference)

On a fresh Sam deployment, before any other wiki operation, seed the
two structural files if they're missing or 0-byte. The skill body
names the mkdir and the two empty-file conditions; this section has
the full seed content so maintainers can verify drift.

### Directory tree

```
/data/workspace/wiki/
  index.md         — catalog of pages with one-line summaries
  log.md           — append-only operations log
  sources/         — immutable raw documents (never modify after saving)
  entities/        — people, projects, systems, services, vendors
  concepts/        — technologies, decisions, design patterns, topics
  syntheses/       — cross-cutting analyses spanning multiple entities
```

### `index.md` seed content

```markdown
# Wiki Index

Catalog of every page in the wiki with a one-line summary.
Keep entries sorted by category, then alphabetically within
category.

## Entities

## Concepts

## Syntheses
```

### `log.md` seed content

```markdown
# Wiki Operations Log

Append-only record of ingest, query, and maintenance operations.
Newest entries at the bottom.
```

### 0-byte state

Pre-v2.0 deployments created the directories but skipped seeding. If
`index.md` or `log.md` exists but is 0 bytes, run bootstrap before
doing any other wiki operation. Writing `Related:` links against an
empty index later will silently orphan cross-references.

## Page template — full with worked example

The skill body has the bare structural template. Here's the annotated
version plus a real entity page for reference.

### Template

```markdown
# <Human-readable title>
Category: entity | concept | synthesis
Created: YYYY-MM-DD
Last updated: YYYY-MM-DD
Related: [[other-page-slug]], [[another-page-slug]]

## Summary

One paragraph. What is this page about and why does it exist?

## Details

### YYYY-MM-DD
- First fact or observation about this entity or concept.
- Second fact, with citation to the source if it came from an
  ingest (e.g., "from meetings-2026-04-12.md").

## Open Questions

- Gap: Unresolved question, prefixed for lint to catch.
```

### Real example — `dan-jacobsen.md`

```markdown
# Dan Jacobsen
Category: entity
Created: 2026-04-12
Last updated: 2026-04-13
Related: [[uptempo-positioning]], [[speakr]]

## Summary

Senior Product Manager and primary collaborator on the Uptempo
re-engagement strategy.

## Details

### 2026-04-12
- Involved in JMS re-engagement strategy (from meetings-2026-04-12.md).
- Managing sprint transitions and task visibility for the team.

### 2026-04-13
- Confirmed Q3 positioning decision: coexistence over replacement.

## Open Questions

- Gap: Who owns the customer comms rollout timeline?
```

### Slug rules

- Lowercase, hyphens for spaces, no special characters.
- Filename without `.md` and without directory.
- Use the slug in `[[link]]` cross-references; Sam resolves the
  directory from the page's `Category:` field, not from link syntax.

### Cross-reference reciprocity

If page A links to B in its `Related:` header, B should also list A in
its `Related:` header. Keeps the wiki navigable in either direction.
Lint flags one-way references (see Lint operation).

There is no runtime wiki-link resolver — `[[slug]]` links are plain
text. Treat them as documentation of intent, not navigation.

## Lint cron — reference block

The `wiki-lint` cron is optional and not bootstrapped by default. If
Dan wants daily lint, create via `cron_add`:

- name: `wiki-lint`
- schedule: `0 9 * * 1-5`
- timezone: `America/Vancouver`
- job_type: `agent`
- session_target: `isolated`
- delivery: `{"mode":"none","best_effort":true}` (Sam's
  `send_user_message` with default recipient handles any findings)
- prompt:

```
You are in an isolated cron session for wiki-lint.
Run the wiki-management skill's Lint operation.
Do not call cron_run — you are already inside the cron job.
If the wiki is clean, exit silently with a one-line status.
If there are issues, summarize and call send_user_message.
```

## Extended failure modes

Covered in the skill body (brief): path-absolute reminders,
`file_edit` anchor failure, 0-byte bootstrap state, mid-turn provider
timeouts. Rare cases below:

- **`glob_search` returns a match in both `entities/` and `concepts/`
  for the same slug** → naming collision. Pick the directory matching
  the `Category:` of the real content; rename the other page to
  disambiguate. Log the rename in `/data/workspace/wiki/log.md`.
- **`send_user_message` fails during Lint** → fall back to writing a
  summary into `/data/workspace/wiki/log.md` so the lint result isn't
  lost; surface on the next interactive turn.
- **`content_search` returns nothing for a topic you're sure is
  indexed** → the search path must be absolute. Relative paths silently
  return nothing.
- **Synthesis page proliferation** → only create a synthesis page when
  a source connects multiple pre-existing entities or concepts in a
  way none of them individually captures. Most sources are per-entity
  updates; trivial syntheses dilute the wiki.

## History

- **v2.2.0 (2026-04-22)** — split into lean SKILL.md + this design
  doc. Removed 13-line absolute-paths explanation, 23-line
  context-hygiene paragraph, full bootstrap file contents, concrete
  page-template example, and extended failure-mode narration. Body
  went from ~380 lines to ~180. Same goal as the paired
  `daily-meeting-summary` v4.3.0 split: stop Qwen 3.6 35B from
  exhibiting attention-falloff truncation on the tail of long
  tool_result content.
- **v2.1.0 (2026-04-13)** — absolute paths enforced everywhere. Prior
  versions allowed relative paths that silently landed in the wrong
  tree.
- **v2.0.0** — wiki restructured into `entities/`, `concepts/`,
  `syntheses/` buckets with the current `index.md` + `log.md`
  structure.
