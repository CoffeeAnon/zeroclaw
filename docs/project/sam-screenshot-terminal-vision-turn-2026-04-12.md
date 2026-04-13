# Sam Screenshot Terminal Vision Turn — RCA 2026-04-12

## Summary

After the 2026-04-10 disk-read fix (`c287fbd4`) correctly delivered screenshot payloads to the multimodal pipeline, Sam still produced `Invalid url value` errors from Gemma 4 / LiteLLM on the immediate post-screenshot LLM request. This document records the root cause and the targeted remediation.

## Validated Failing Production Request Shape

After a browser-screenshot tool call, `run_tool_call_loop` assembled the following request at the next iteration:

1. `[tool role]` — screenshot tool result, content contains `[IMAGE:data:image/...;base64,...]` (converted to `user` multimodal message on the wire by `OpenAiCompatibleProvider`)
2. `[user role]` — safety heartbeat: `[Safety Heartbeat — round N/M]\n<body>` (appended unconditionally)
3. Native `tools` / `tool_choice` schema

This produced two consecutive `user`-role messages on the wire. Gemma 4 / LiteLLM rejected consecutive user messages, returning `Invalid url value`.

## Validated Working Direct Probe Shape

A direct probe to the same LiteLLM endpoint with:

1. `[user role]` — one multimodal message with the screenshot (`image_url` content part)
2. No trailing `user` heartbeat
3. No tool schema

…succeeded and returned a description grounded in the image.

## Causal Analysis

Two structural differences between the failing production request and the working direct probe were identified:

| Difference | Status |
|---|---|
| Trailing safety heartbeat (`user` role) appended after the screenshot turn | **Confirmed causal** — removing it fixes the error |
| Native `tools` / `tool_choice` present | Not yet isolated — may contribute but not tested in isolation |

The heartbeat was the primary target because it was the clearest consecutive-user-message source. Tool-schema suppression is deferred as a follow-up experiment.

## Fix Applied

**File:** `src/agent/loop_.rs`

Added helper `request_contains_screenshot_followup_turn(messages: &[ChatMessage]) -> bool` that scans the assembled `request_messages` for a `tool`-role message whose content (including JSON-wrapped `{"tool_call_id":…,"content":…}` form) contains an `[IMAGE:]` marker.

Gated heartbeat injection:

```rust
let has_screenshot_followup = request_contains_screenshot_followup_turn(&request_messages);
if let Some(ref hb) = heartbeat_config {
    if should_inject_safety_heartbeat(iteration, hb.interval) && !has_screenshot_followup {
        request_messages.push(ChatMessage::user(reminder));
    }
}
```

Scope of the fix:
- Only the immediate screenshot-bearing follow-up request is affected.
- Subsequent iterations (after the screenshot analysis turn is complete) receive heartbeats normally.
- `tools` / `tool_choice` are preserved in the initial fix so screenshot-driven tool workflows still function.

## Tests Added

**`src/agent/loop_.rs`:**
- `screenshot_followup_request_skips_safety_heartbeat` — integration test through `run_tool_call_loop` with `ScreenshotTool` + `RequestCaptureProvider`; asserts no `[Safety Heartbeat` in the second provider call
- `request_contains_screenshot_followup_turn_detects_json_wrapped_tool_image` — unit test
- `request_contains_screenshot_followup_turn_false_without_image` — unit test
- `request_contains_screenshot_followup_turn_false_for_user_image_only` — unit test (user-role images must not suppress heartbeat)

**`src/providers/compatible.rs`:**
- `screenshot_followup_request_keeps_user_multimodal_image_content` — regression: screenshot tool result serializes as `user` multimodal with `image_url` content parts
- `screenshot_followup_request_preserves_tool_schema_when_present` — regression: provider does not silently strip `tools` from the screenshot follow-up request

## Rollback

Revert `fix(agent): suppress heartbeat on screenshot follow-up` if the suppression regresses safety-policy reinjection for screenshot-driven tool chains.

The suppression is narrowly scoped (one iteration, one condition) and the commit is standalone — revert is clean.

## Deferred

Tool-schema suppression on the screenshot follow-up turn is not part of this fix. If the heartbeat-only remediation still reproduces `Invalid url value` in a follow-up experiment, tool-schema suppression should be tested as a separate commit with its own rollback path.

## Prior Art

- `sam-screenshot-tool-image-rca-2026-04-10.md` — disk-read path fix for path-only screenshot results
- `sam-screenshot-context-lifecycle-2026-04-11.md` — multimodal context lifecycle and history downgrade
