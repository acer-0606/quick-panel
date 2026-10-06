# Pi Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace one-shot organizer inference with a bounded Pi tool loop, durable host memory/checkpoints, human feedback and ready Windows distribution.

**Architecture:** Pi Agent Core runs in an on-demand Node process with six registered business tools and a Rust-proxied Claude transport. One Rust actor owns source validation, memory, task checkpoints and low-frequency transactions; native UI routes correlated calls without blocking.

**Tech Stack:** Existing Rust/serde/rusqlite/Win32/WinHTTP; pinned @earendil-works/pi-agent-core1.0.4 and pi-ai1.0.4, Node24.19.0, bundled ES module. No general agent CLI or Pi Durable runtime.

**Spec:** `docs/superpowers/specs/2026-10-07-pi-harness-design.md`

## Global Constraints

- 管家默认关闭；Suggest 默认后台生成建议，Automatic 只填空别名并合并标签。
- 沿用四种范围、间隔30–3600秒（默认60）、每批1–8（默认4）、说明512字符、上下文1024字符。
- Pi最大模型轮次可配置2–8（默认4）；每轮任务工具调用最多32次，总时限120秒；一次只有一个真实任务进程。
- IPC协议版本1；每帧最多256KiB，LLM请求最多192KiB、响应64KiB；工具返回最多16KiB，检查点最多128KiB、最多64条消息。字节超限有终态，不继续无界读取。
- 原整理缓存保持256条/2MiB、Ready128条；新增记忆/任务存储独立预算1MiB、记忆最多256条，其中已确认规则最多64条，一份当前检查点及暂存提交。先淘汰较旧普通示例，不静默淘汰已确认规则。
- 自动队列、检查点、AI生成的待确认规则沿用约55秒开始/健康情况下60秒批量事务；人工保存、拒绝/修改反馈、记忆编辑/删除、设置即时提交；正常退出保存已接收缓存。
- 会话不写JSONL文件，Node不写任务日志；同一源码构建后的bundle不动态加载用户目录或配置。
- 取消/配置变更撤销结果；实际运行结束前不释放占用。UI及SQLite actor不做网络或等待子进程，退出不等待模型。
- 无真实API、用户凭据/正文/剪贴板/屏幕读取。Windows原生测试只交叉编译。
- 使用原项目目录、codex/quick-panel分支；Cargo串行，固定CARGO_INCREMENTAL=0/CARGO_BUILD_JOBS=1/CARGO_PROFILE_DEV_DEBUG=0/CARGO_PROFILE_TEST_DEBUG=0。

---

### Task 1: Pi runtime and shared bounded protocol

**Files:** Create `agent-runtime/package.json`, pinned lockfile, `agent-runtime/src/worker.mjs`, `agent-runtime/test/worker.test.mjs`, bundle build script, `src/pi_harness.rs`, `tests/pi_harness_protocol.rs`; modify `src/lib.rs`, `.gitignore` and add dependency notices needed by this runtime. Own this task only; no engine or native implementation.

**Interfaces:** Produce serializable protocol-v1 HostFrame/RuntimeFrame, ToolName (the six spec names), ToolError {code,message,retryable}, bounded Checkpoint {messages_json,rounds,calls}, model request {system,messages,tools,max_tokens}. Start carries request_id, model, public effort, instructions, item_refs(index/title/kind/version), max_rounds and optional checkpoint; no key/config directory. Every outgoing request has request_id/call_id. Replies are structured value or ToolError. Runtime produces ready, llm, tool, checkpoint, completed or failed frames. Record exact Rust and JS signatures in report before Task2 begins.

- [ ] Write and run tests for real Pi tool-call→host error→model correction→submit, refusal/incomplete result, arbitrary final chat without submitted outputs, resume from settled transcript, duplicate replies, unknown tools, byte/message/round/call limits and no filesystem/session writes. Rust tests validate malformed/oversized frames and checkpoint bounds. Use real Agent with a controlled LLM bridge; do not test a fake Agent.

```rust
#[test]
fn protocol_rejects_unknown_operation_and_oversized_checkpoint() {
    assert!(quick_panel::pi_harness::parse_runtime_frame(br#"{"type":"shell","command":"x"}"#).is_err());
    let bytes = vec![b'x'; 256 * 1024 + 1];
    assert!(quick_panel::pi_harness::parse_runtime_frame(&bytes).is_err());
}
```

- [ ] Implement the actual Pi Agent using supplied streamFn, only registered business tools, sequential host callbacks, explicit counters and a hard task deadline. Convert structured Anthropic content/tool_use/tool_result blocks at the transport boundary; ordinary text cannot become metadata. Surface thrown tool errors to Pi so it can correct. Checkpoint only replay-safe settled transcript boundaries; keep counters on resume. Resolve successful completion through host-accepted submissions and a structured completion receipt.

```javascript
// The production execution path must use Pi's Agent, not a replacement loop.
import { Agent } from '@earendil-works/pi-agent-core';
const agent = new Agent({streamFn: transportBridge, toolExecution: 'sequential'});
agent.state.tools = registeredBusinessTools;
```

- [ ] Pin package versions and lock dependency resolution, install with scripts disabled, build one self-contained module using only required Pi exports. Verify the bundle runs with real Node24.19.0 on controlled stdin/stdout, contains no API credentials, and exits on completion/EOF/cancel. Report bundle size, transitive licenses and actual package APIs.
- [ ] Run Node tests, focused Rust protocol tests, host Clippy and Windows all-target check. Commit only task files; report exact test evidence and integration contract. Run the full Rust suite only if new contracts affect existing behavior; otherwise Task3 final full covers it.

### Task 2: Actor tools, memory, checkpoints and human feedback

**Files:** Create `src/harness_store.rs`, `src/harness_tools.rs`, `tests/harness_engine.rs`; modify `src/engine.rs`, `src/model.rs`, `src/organizer.rs`, `src/organizer_store.rs`, `src/repository.rs`, `src/lib.rs`, relevant fixtures and minimal native event compatibility arms. No process/network/UI implementation in this task.

**Interfaces:** Consume Task1 frame/types. Add `HarnessContext {token,request_id}` → `HarnessContext {token,request_id,result}`, `HarnessTool {token,call_id,name,arguments}` → `HarnessToolResult {token,call_id,result}`, `HarnessCheckpoint {token,checkpoint}`, `HarnessFinish {token,request_id}` → `HarnessFinished {token,request_id,result:Result<Vec<Proposal>,String>}`. Finish validates staged full coverage but does not apply twice: returned proposals follow the existing OrganizerComplete lifecycle once. Add `HarnessFeedback {request_id,sources,reason,scope}`, `HarnessMemorySnapshot {request_id}`, `HarnessMemoryEdit {request_id,input}`, `HarnessMemoryDelete {request_id,ids}` with correlated state/save receipts. Concrete context/memory/input structs and namespaces belong to Task2 report. Keep OrganizerWork as source snapshot or extend via an explicit wrapper documented in report. Memory scope is item/directory/global; global/directory authority is human-confirmed. Manual Retry may replenish the bounded runtime attempt budget; automatic restart may not reset it.

- [ ] Write real Engine/SQLite RED tests for tool-submit validation and repairable errors, source/version/disabled guards, memory scope/negative alias, user correction versus AI output, submit/finish idempotence, checkpoint/restart/profile mismatch, bounded caches, lazy optional tables, no writes on reads, automatic deadline and immediate human transactions.

```rust
// Observable flow: submit an overlong alias -> retryable tool error ->
// submit a valid alias -> finish -> Ready proposal. No network-failure count
// is spent, original target/body remains unchanged, read data_version stays.
```

- [ ] Implement a bounded memory/checkpoint store with repository `write/committed` lifecycle in the same transaction as source metadata. Return typed errors for stale run/source, invalid arguments, rejected alias and capacity. Bind index to active Work; only the original batch can be submitted. Staged results survive checkpoint recovery; finish validates complete coverage and current sources before legacy auto/Ready application.
- [ ] Implement context/search/recall/usage as bounded read projections without file/BLOB access. Recall exact item/confirmed directory/global scopes then keyword examples, limit8/8192chars. Do not add automatic embeddings or infer global authority from single rejection. Capture accepted/edited/rejected examples only from trusted UI commands; auto application is explicitly AI-origin evidence.
- [ ] Add memory_enabled(defaulttrue with parent disabled) and max_rounds(default4,2–8) to organizer settings with safe serde defaults. Explicit feedback-and-redo persists reason/scope, invalidates old work, queues same source with old checkpoint cleared; ordinary defer does not learn a reason. Memory edits/deletes have immediate correlated save receipts and retain failed edits.
- [ ] Run focused engine/store/protocol/organizer regressions; full shared suite once, strict host Clippy and Windows cross-check. Commit and report concrete commands/events and memory budgets for Task3. Intermediate native arms may explicitly absorb new events, but Task3 must replace them before delivery.

### Task 3: Native runtime lifecycle, feedback UI and distribution

**Files:** Create `src/harness_worker.rs`, `src/platform/windows/harness_memory_ui.rs`, relevant portable lifecycle tests and packaging/runtime-download scripts; modify `src/platform/windows/ui.rs`, `src/platform/windows/ai_client.rs`, `src/platform/windows/dialogs.rs`, `src/platform/windows/organizer_ui.rs`, `src/organizer_ui.rs`, `src/platform/windows/mod.rs`, Cargo Windows features if job-object cleanup needs them, README, verification and third-party notices. Consume reviewed contracts rather than redefining them.

**Interfaces:** Consume Task1 bundle/protocol and Task2 commands/events. One process worker handles the LLM bridge on its background thread and one actor tool request/reply slot. UI sends actor commands and returns replies; checkpoint updates coalesce. Host never interprets final assistant text as an action.

- [ ] Write portable real-child/real-channel RED tests for handshake/terminal failure, one actual occupied job, cancel/timeout/EOF/malformed frames, bounded stdout/stderr, late token/call replies, spawn-free disabled state, checkpoint merge, shutdown without waiting and owned child cleanup. No real model API or user content.

```rust
// A controlled child emits one request and blocks. Cancel must reject its late
// result and clean up the child, while the GUI remains free to process events.
// A second task cannot start until the first child has really terminated.
```

- [ ] Implement hidden Node process using bundled absolute paths, cleared Node preload options and explicit protocol. Use WinHTTP with existing protected credentials for llm frames, strict body/tool/name bounds and existing effort. Rust source validation stays in actor. Add Windows-owned job cleanup if needed; closing native UI never joins the process or network. Missing runtime/panic/transport error has localized terminal state.
- [ ] Replace OrganizerWorker's one-shot call with Pi execution and the real actor bridge; preserve main-list status/order/preview/IME/source guards. Bind all results to settings+token, ensure parent exit saves cache and releases owned process. Settings show memory switch/max rounds and runtime health test which does not call a model.
- [ ] Add feedback-and-redo form (reason and explicit scope), ordinary defer, confirmed edited/accepted feedback, and memory manager with view/edit/confirm/delete and correlated save errors. Reuse Chinese native theme, DPI, keyboard focus and scroll. Process only completed local suggestions on confirmation.
- [ ] Bundle pinned Node24.19.0 Windows x64 from official release with SHA256 verification, real Pi module, licenses and current docs. Add fresh-ZIP packaging, CRC/member-byte/SHA256 checks; no global installs or dynamic downloads at user startup. Report package increase and owned synthetic runtime startup/memory measurements with platform limits.
- [ ] Run Node suite, focused lifecycle/native regressions, final shared Rust suite once, strict host/Windows all-target Clippy, fmt/diff, Windows fixtures cross-compile, Release and core-check. Record real local transport fixture versus Windows-only compilation distinctly. Commit/report; final whole-feature review begins after all three tasks pass.
