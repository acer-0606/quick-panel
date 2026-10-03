# Optional Claude AI Search Implementation Plan

> **For agentic workers:** Use subagent-driven-development for isolated protocol, native transport and engine tasks; root integrates the UI. Track completed checks here.

**Goal:** Opt-in natural-language search through Claude Messages API, with real local results and no additional history writes.

**Architecture:** A bounded on-demand network worker produces a validated query plan. The existing single-writer engine performs multiple local keyword searches against SQLite and the pending overlay. The native panel merges known function suggestions and real saved items.

**Tech Stack:** Existing Rust/serde/SQLite, Windows WinHTTP and DPAPI; no new runtime or model dependency.

**Spec:** ../specs/2026-10-03-optional-ai-search-design.md

## Global constraints and contracts

- AI disabled by default, manual invocation only, user-configured Claude model/endpoint.
- Secrets are DPAPI ciphertext in configuration; no request/history logging.
- Claude POST /v1/messages, anthropic-version 2023-06-01, top-level system, non-streaming text JSON plan.
- Core AiSettings/AuthMethod/AiPlan/Period/FunctionDescriptor in src/ai.rs; root owns these contracts.
- AiPlan: queries Vec<String>, scope Option<Scope>, period Period, functions Vec<String>.
- AiSearchQuery: queries Vec<String>, scope Scope, from_ms Option<i64>, until_ms Option<i64>, request_id u64. Added as Command::AiSearch and emits ordinary Event::Results.
- Responses, requests and query plans are bounded. Function suggestions never execute automatically.
- Existing hotkeys, IME, modal borrow safety, selected identity, low-write policy and exit flushing must survive.

## Task 1 Claude protocol — owner claude_protocol

Owned src/claude.rs and tests/claude.rs. Root adds lib module declaration.

- [x] Write behavioral tests for URL normalization, malformed credentials/headers, real Messages wire shape, strict plan parsing, truncated/error responses and unknown function IDs; run before implementing behavior.
- [x] Implement `parse_endpoint(&str) -> Result<Endpoint, String>`, `prepare(&AiSettings, &str, &str, Scope, &[FunctionDescriptor]) -> Result<HttpRequest, String>` and `parse_response(u16, &[u8], &[FunctionDescriptor]) -> Result<AiPlan, String>`.
- [x] HttpRequest exposes endpoint host/port/path/secure, headers Vec<(String,String)> and body Vec<u8>; Debug must redact header values/body. Ensure bounds 2048 query chars, 6 terms of 64 chars, 3 known functions, 64 KiB response.
- [x] Test fixtures include `{"content":[{"type":"text","text":"{\"queries\":[\"连接超时\",\"connection timeout\"],\"scope\":\"clipboard\",\"period\":\"yesterday\",\"functions\":[]}"}],"stop_reason":"end_turn"}` and `max_tokens` / tool_use rejection. Only named allowed fields and scope/period values accepted.
- [x] Run targeted tests, write report with actual commands and outcomes. No Git operations.

## Task 2 Native HTTP and secret protection — owner claude_native

Owned src/platform/windows/ai_client.rs and tests/windows_ai.rs. Root enables Networking_WinHttp and Security_Cryptography and declares module.

- [x] Implement `protect_key(&str) -> Result<Vec<u8>, String>` and `unprotect_key(&[u8]) -> Result<String, String>` using user-level DPAPI and owned/freed buffers.
- [x] Implement `interpret(&AiSettings, &str, Scope, &[FunctionDescriptor]) -> Result<AiPlan, String>` using Task 1 prepare/parse_response; synchronous WinHTTP is called only by the AI worker.
- [x] Use RAII handles, bounded connect/send/receive timeouts, no redirect following, no invalid-certificate override, 64 KiB response cap, generic redacted errors and no automatic request retries.
- [x] Add Windows-only local HTTP fixture tests for actual POST/headers/body/error/size behavior and DPAPI round-trip; do not use a real API or read environment secrets. Native execution unavailable here must remain explicit.
- [x] Cross-target compile after interfaces are ready; report evidence and limitations. No Git operations.

## Task 3 Local expanded query — owner ai_engine

Owned src/engine.rs, src/repository.rs, tests/ai_search.rs.

- [x] Write real SQLite regressions for two synonym queries and deduplication, scope/time filtering before candidate quotas, pending visibility and zero commits during search. Example: 600 newer matching rows must not hide an older matching row in a requested date range.
- [x] Refactor normal search minimally into a reusable result collector, keeping all original ranking and metadata/BLOB behavior.
- [x] Extend repository summaries with optional creation-time bounds while leaving the unfiltered normal SQL path equivalent. Apply filters to all candidate streams before LIMIT; retain literal validation and keyset paging.
- [x] Implement Command::AiSearch with 6 bounded queries, empty query allowed for explicit date/scope browse, union by real ID, bounded rank/sort/50 results and pending overlays. Reject malformed ranges and oversized untrusted query plans.
- [x] Test existing persistence/search regressions and report. Do not edit root-owned model or UI, do not stage/commit.

## Task 4 Native settings, jobs and panel — root

Owned src/ai.rs, src/ai_jobs.rs, src/model.rs, src/lib.rs, Cargo.toml, Windows mod.rs/ui.rs/dialogs.rs and related palette/ui state tests.

- [x] Declare contracts and extend Settings with serde-default AiSettings; old settings must remain disabled.
- [x] Add bounded lazy AI worker with cancellation/stale-result tests. A blocked request must not block local queries or cache shutdown.
- [x] Add settings navigation to a separate native AI form with password field, enable, endpoint, model, auth and clear-key controls. Validate enabled settings before saving; preserve ciphertext on blank input.
- [x] Add searchable AI configuration and an enabled-only “按意思查找” button; reuse existing panel key/menu handling. No new global key or automatic AI requests.
- [x] Resolve supported period boundaries in Windows local time, submit AiSearch and merge only validated known functions. Keep AI plan in memory and rerun local search, not network, when history changes.
- [x] Fix tests, run fmt/full tests/macOS+Windows strict Clippy/core-check/Release build; independent review and scoped fixes.
- [x] Update README/verification with configuration, payload scope, bounded cancellation and runtime boundaries; package, inspect ZIP/current EXE hash and commit local reviewed source. No remote publishing.

## Completion — 2026-10-03

Product source b5658ba. Root fresh125 shared tests and both strictClippy checks passed;2 prior heavy regressions ignored,13 Windows-only native tests compiled but not run. Windows and macOS Release builds/core-check passed; fresh10k/100k core p95 9.048/39.330ms. Final independent review and fix scope review clear; accepted-plan local token/cache revocation and sensitive config validation were fixed. The subsequent nonblocking config-feedback observation uses the existing owned Message effect so it remains readable. See docs/verification.md for limits and artifact identity. Future AGENTS/MCP/RAG discussion is documented as unimplemented proposals.
