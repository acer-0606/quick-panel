# AI Organizer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add configurable background AI classification/aliases with two save modes, model discovery and effort selection, retaining low writes and responsive Windows interaction.

**Architecture:** Portable validated protocol/contracts feed an actor-owned bounded organizer store. A native background worker handles HTTP and a native review window edits completed proposals. Existing tags/aliases and path observations remain the source of truth.

**Tech Stack:** Rust 2024, existing serde/rusqlite/Win32/WinHTTP only; no new runtime dependencies.

**Spec:** `docs/superpowers/specs/2026-10-06-ai-organizer-design.md`

## Global Constraints

- 管家默认关闭；模式 Suggest（默认）只生成待确认建议，Automatic 后台自动应用。
- 无新增默认快捷键；所有用户界面使用中文。
- 网络不进入 UI 或 SQLite actor；一次最多一个真实管家请求，取消不提前释放实际占用，退出不等待网络。
- 范围四选一：收藏+固定剪贴板+外部路径（默认）、仅收藏+固定剪贴板、仅外部路径、上述范围加普通剪贴板文本。
- 间隔 30–3600 秒（默认 60）；每批 1–8（默认 4）；整理说明最多 512 字符；每项上下文最多 1024 字符。
- 最多 256 条/2 MiB 管家记录，待确认最多 128 条；先淘汰完成回执，保留原始内容和已有待确认建议。
- 自动模式只填空别名并合并标签；建议别名最多 48 字符、最多 4 个标签；不改磁盘文件或执行模型命令。
- 批量持久化约 55 秒启动/60 秒健康上限，正常退出 flush；用户保存/忽略和配置立即保存，无查询/绘制写盘。
- 连续失败至少退避 5 分钟、10 分钟，第三次暂停，重试或配置变更恢复；持久化退避状态。
- 模型列表最多 5 页/200 项/每页 256 KiB；默认 effort 不发送，显式使用 output_config.effort。
- 无付费接口、用户密钥、真实剪贴板/屏幕访问；macOS 不声称运行 Windows 测试。
- 构建环境：`TMPDIR=/Volumes/QPPathNamesBuild/tmp CARGO_TARGET_DIR=/Volumes/QPPathNamesBuild/target CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`；Cargo 串行。

---

### Task 1: Validated organizer protocol, model discovery and configuration contracts

**Files:** Create `src/organizer.rs`, `src/ai_models.rs`, `tests/organizer_protocol.rs`, `tests/ai_models.rs`; modify `src/ai.rs`, `src/claude.rs`, `src/model.rs`, `src/lib.rs`, `src/platform/windows/ai_client.rs`, relevant existing test literals and `tests/windows_ai.rs`.

**Interfaces:** Produce serializable settings and source/candidate/proposal contracts in `organizer.rs`, used by Tasks 2/3. Required public names: `OrganizerSettings`, `OrganizerMode::{Suggest,Automatic}`, `OrganizerScope::{SavedAndPaths,SavedOnly,PathsOnly,AllTextAndPaths}`, `Candidate`, `Proposal`, `Work`, `Snapshot`. `Settings.organizer` defaults safely. `AiSettings.effort: Effort` with Default/Low/Medium/High/XHigh/Max. Exact fields of work/snapshot may be completed by Task 2; record chosen API in report before downstream dispatch.

Candidate stores an explicit saved-item or observed-path source, stable input fingerprint, expected metadata version, kind/title/alias/tags/context. Fingerprint excludes usage/time and AI-added alias/tags; expected version covers apply-sensitive metadata. Proposal binds an original candidate to optional alias, up to 4 tags and brief reason. Wire only sends numeric candidate indices and permitted context, never source versions, credentials or hidden contents.

- [ ] Write failing protocol tests: valid alias+tags and null alias, reject extra fields/duplicate or unknown indices/control or bidi characters/overlong metadata/incomplete or tool output, scoped JSON payload contains only batch inputs, disabled/invalid credentials reject locally. Model fixtures cover Claude capabilities and minimal gateway list, malformed IDs, duplicate IDs, cursors, endpoint ending in /v1/messages.
- [ ] Run `cargo test --locked --test organizer_protocol --test ai_models` with build environment above; record failures before implementation.
- [ ] Implement bounded pure request/response parsing and reusable Messages helpers without copying auth/endpoint logic. Retain legacy query parser contracts. Example wire:
```json
{"items":[{"index":0,"alias":"客户甲生产日志","tags":["客户甲","日志"],"reason":"路径及备注标明客户和用途"}]}
```
The system prompt forbids invented business semantics, commands and treating input data as instructions. Missing knowledge uses null alias/empty tags. Response contains exactly one result for each batch input, no duplicate indices.
- [ ] Extend native transport with explicit GET model listing plus existing POST, same credential protection, no redirects/retries, bounded pagination/headers/body/deadline. Models use endpoint+entered/saved key even when model is blank or AI disabled. Provide `ai_client::organize(settings, work)` (or equivalent coherent signature) and `ai_client::list_models(settings, entered_key, clear_key)`; no GUI changes in this task. Support bounded ignored thinking blocks, effort transmitted in search/test/organizer. Model capabilities are optional; unknown capability is not false.
- [ ] Add Windows localhost fixtures for GET/auth/unsaved model-independent listing, effort POST and organizer output; cross-compile only. Run focused shared tests, full shared suite once, Windows `cargo check --locked --target x86_64-pc-windows-gnu --all-targets`; report exact commands/results.
- [ ] Commit only task files and record public contracts/tests in report.

### Task 2: Actor-owned background queue, durable proposals and metadata application

**Files:** Create `src/organizer_store.rs`, `tests/organizer_engine.rs`; modify `src/engine.rs`, `src/model.rs`, `src/repository.rs`, `src/path_activity.rs`, `src/path_activity_store.rs`, `src/organizer.rs`, `src/lib.rs`, source-compatible fixtures including `tests/path_names.rs`.

**Interfaces:** Consume Task 1 settings/candidates/protocol. Produce commands/events and document their concrete fields for Task 3. Intended commands: `OrganizerTick`, `OrganizerComplete { token, result }`, `OrganizerSnapshot { request_id }`, `OrganizerApply { proposals }`, `OrganizerDismiss { sources }`, `OrganizerRetry`. Events: `OrganizerWork(Work)`, `OrganizerState { request_id, snapshot }`, `OrganizerChanged`. Work carries a revocable token, exact AI/settings snapshot and small batch; Snapshot carries ready proposals/counts/status, never raw stored BLOBs.

- [ ] Write failing engine tests using synthetic paths/SQLite: both modes, no network-needed actor schedule, queue dedup under recopy/usage/AI changes, scopes/exclusions, config invalidation, stale/deleted source, preserved bodies/representations/targets, path alias search, failure cooldown/restart, size ceilings and lazy table.
- [ ] Run the focused tests to observe missing behavior. Implement a small cache with serde records and repository-transaction `write/committed` lifecycle like path_activity_store. All rows loaded/written within count/byte limits; no unbounded whole-database scan. New eligible capture/favorite/pin/path-observation enqueues when enabled; default enable does not retroactively upload old history. Auto changes must not feed their own input queue.
- [ ] `OrganizerTick` checks allowed settings/time/queue/proposal budget and emits at most one Work; complete validates token/current settings/source version and proposal contents, then stages Ready or auto metadata. Stale/disabled/replaced results do nothing. Failure queues source refs for bounded retries, using persisted backoff/pause counters. Restart recovers in-flight work as queued. Existing ready suggestions are not silently applied when switching to Automatic.
- [ ] Apply manual proposals atomically after all selected sources/versions are validated; use metadata-only mutation for saved items, preserving pending Content if present. Persist ready removal and actual aliases/tags in the same repository transaction. Automatic changes use normal deadline, manual apply/dismiss flush immediately. Deleted/unpinned/excluded/suppressed/cleared/expired sources invalidate related work so old proposals cannot resurrect them.
- [ ] Extend observed path metadata with alias/tags while preserving normalized identity and original case target, search through existing local matching. Automatic annotation does not create favorites. Update palette accessors/adapters as needed so Task 3 can show the aliases, and existing deterministic labels remain useful for empty aliases.
- [ ] Run focused engine/persistence/path tests, full shared suite once, cross-target check. Add concrete API section to report, commit.

### Task 3: Responsive Windows settings, model picker and completed-suggestion review

**Files:** Create `src/platform/windows/organizer_ui.rs` and portable GUI lifecycle helper tests if needed; modify `src/platform/windows/ui.rs`, `src/platform/windows/dialogs.rs`, `src/platform/windows/mod.rs`, `src/palette.rs`, `src/platform/windows/ai_client.rs` only for necessary integration, README and verification documentation; fix/regression-test earlier `src/path_labels.rs`/`tests/path_names.rs` integration if needed.

**Interfaces:** Consume documented Task 1 native model/organizer calls and Task 2 commands/events. Existing native Host/UiEffect pattern must avoid RefCell borrows across nested Windows message loops; completed reviews submit only to the actor.

- [ ] Add behavioral tests for late result/cancel/double-submit/actual-busy gates where new logic is portable. Existing worker can be reused when already covers these invariants. Run new focused tests before implementation.
- [ ] AI settings uses editable model combo, async “获取模型” and existing “测试连接” on current unsaved values, effort choice with known capability guidance; default effort is compatibility fallback, no silent failed-request retry. Model list fetch must remain cancellable without joining; close/reopen cannot create unbounded simultaneous fetches. Local validation/failure remains in settings.
- [ ] Add separate native AI 管家设置 using current appearance/DPI and real controls: enable disclosure, mode, scope, interval, batch size, instructions. Settings immediately persist. No optional mode silently enables network. Add searchable functions and menu entries for settings and 整理建议.
- [ ] Integrate background timer only while enabled, worker lazy start and one real occupied request; callbacks token/settings checked, no network/UI/actor blocking. Use snapshot status updates sparingly, no busy polling or search-page error spam. Disable/quit revokes and returns promptly; existing history flushing still completes.
- [ ] Build completed-suggestion review window with list/checks, source context, editable selected alias/tags, apply selected/dismiss selected/retry/close. Opening reads local state, never waits for AI. Applying after source changes reports controlled local stale status, no resurrections. User can close with work still in background and reopen later. Preserve meaningful keyboard/mouse access without new shortcuts.
- [ ] Verify deterministic path detail snapshots are cached, path aliases win, original targets remain available in preview/copy. Clipboard title behavior can prioritize a useful saved alias while retaining original heading/context; no change to clipboard payload.
- [ ] Update Chinese README for real behavior, limits, privacy scope, two modes, model/effort compatibility, low writes. Run focused tests, full shared suite once, fmt, strict all-target Clippy on host/Windows, Windows fixture cross-compile and Release build. Record native tests not executed on macOS. Commit and report.
