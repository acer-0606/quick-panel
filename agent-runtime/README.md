# Quick Panel Pi runtime

One task per Node process. The production bundle executes the pinned Pi Agent Core `Agent` (1.0.4), with sequential business tools and a Rust-proxied `streamFn`. It does not load a provider, credentials, coding-agent CLI, global configuration or disk sessions.

Use Node 24.19.0 and pnpm 11.25.0:

```sh
pnpm install --frozen-lockfile --ignore-scripts --store-dir .pnpm-store
pnpm build
pnpm test
```

`dist/worker.mjs` is self-contained except Node's `node:crypto`. Ship it with `THIRD_PARTY_NOTICES.txt` and a verified Node runtime; `node_modules` is build input only. The build asserts the emitted dependency whitelist. `licenses/dependency-inventory.json` records installed locked dependencies; only Pi Agent Core, Pi AI utility functions and TypeBox appear in the distributed code.

Protocol is UTF-8 JSON with one frame per line, version 1. Type definitions and bounded reader are in `src/pi_harness.rs`. Matching fixture: `fixtures/protocol-v1.json`; regenerate via `node scripts/capture-fixtures.mjs`. Protocol payloads never include API keys, endpoints, configuration paths or source database IDs. Item versions are opaque strings.

Start once, then respond to each `llm`, `tool` or `finish` with the corresponding `_result`, exact `request_id` and `call_id`. Only one request can await a reply. Call IDs include a process incarnation UUID to prevent cross-process late replies. Success: `{"ok":true,"value":...}`; error: `{"ok":false,"error":{"code":"...","message":"...","retryable":true}}`. Retryable tool errors are thrown to Pi and replayed as `is_error:true` tool results. LLM errors preserve host code/retryability and end the attempt.

LLM value is a normalized Anthropic response containing exactly `content` and `stop_reason`. Supported blocks: text, tool_use, thinking/signature, redacted_thinking/data. Requests emit tool_result blocks. The host projects these fields from the raw API response and keeps authentication/routing/usage local. It must validate model/effort against its dispatched work, not change routing from Node input.

Successful organizer_submit values must include `accepted:true,index`. They mean admitted/staged, not saved. Only complete coverage requests `finish`; the host validates the staged batch and responds with `accepted:true,submitted_indices:[...]`. `completed` is emitted only after exact receipt coverage. The host then follows its existing OrganizerComplete application/save flow once. Text never produces a proposal.

Checkpoint contains only a settled Pi transcript (`messages_json`) and cumulative `rounds`, `calls`, `elapsed_ms`. Whole tool batches settle before transcript checkpoints; budgets are reserved before requests/tool execution. The host binds checkpoints to task identity, model, effort, settings, original sources and versions, stores only the latest through its existing transaction cadence, and owns submit deduplication. An automatic restart cannot reset budgets. Downtime/backoff is excluded from active time. A crash may replay unpersisted progress; no API billing deduplication is promised.

Fixed limits: frame 256 KiB; model request 192 KiB; response 64 KiB; tool value 16 KiB; checkpoint 128 KiB / 64 messages; 32 attempted tool calls; 120 seconds active execution; max_rounds 2–8, default 4. No transcript trimming resets counters. EOF, cancel, invalid correlation and budget violations end with `failed` and exit 1. Success exits 0 after the final frame is flushed. A destroyed output pipe or forced OS kill cannot deliver a frame, so the host must treat exit without terminal receipt as runtime failure and clean up the owned process.
