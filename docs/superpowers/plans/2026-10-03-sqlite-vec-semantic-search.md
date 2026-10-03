# sqlite-vec Semantic Search Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现默认关闭的 sqlite-vec 语义检索，使用独立配置的 OpenAI 兼容 Embedding API。

**Architecture:** 可移植协议和有界网络 worker 生成向量；存储 actor 独占 SQLite 和有界待提交向量；Windows UI 按需发起查询或逐批索引。词法即时结果保留，只有明确触发的语义查询调用网络，已完成的派生索引沿用原有批量保存和退出流程。

**Tech Stack:** Rust 2024、rusqlite 0.40.2 bundled、固定非 alpha sqlite-vec crate、WinHTTP、DPAPI、Win32 原生控件。

**Spec:** docs/superpowers/specs/2026-10-03-sqlite-vec-semantic-search-design.md

## Global Constraints

- 默认关闭；与 Claude 的地址、模型和密钥独立；不新增全局快捷键。
- OpenAI 兼容 `/v1/embeddings`，Bearer Key，`model`、字符串数组 `input`、`encoding_format: "float"`；每批最多 16 条、4096 字符/条、请求 512 KiB、响应 2 MiB、有限非零 f32 向量维度 1..=4096。
- 配置和收藏立即保存；首次脏数据约 55 秒开始提交、健康情况下 60 秒窗口；向量缓存预算 4 MiB；空闲没有变更时不提交；退出保存所有已接收向量且不等待网络。
- 不上传图片、representations BLOB 或文档正文；文档/文件夹仅已保存元数据。默认索引固定内容与收藏，可选择全部文本；建立/更新索引由用户主动触发。
- 单一存储 actor 独占 SQLite；每个网络 worker 最多一个在途任务、一个待处理任务和一个结果槽；不自动重试付费 POST。
- 不混用 profile/维度；批次接收重新校验当前内容哈希；候选分页检查最多 128 条、输出最多 16 条；已删除/改变的旧条目不能复活。
- 范围、当前索引范围和删除/内容变更排除在最近邻截断前生效；内存原文/向量参与查询；最终最多 50 条。
- 不用用户密钥或付费 API 测试；区分 macOS 共享测试、Windows 交叉编译和真正实机运行。

---

### Task 1: Portable settings, protocol and bounded network worker

**Files:**
- Create: `src/semantic.rs`, `src/embedding.rs`, `src/embedding_jobs.rs`
- Modify: `src/model.rs` (Settings only), `src/lib.rs`, `src/claude.rs` (shared endpoint normalization only)
- Test: `tests/embedding.rs`, `tests/embedding_jobs.rs`, existing `tests/claude.rs`

**Interfaces:**
- Produces `EmbeddingSettings { enabled: bool, endpoint: String, model: String, encrypted_key: Vec<u8>, index_scope: IndexScope }`, serde defaults and redacted Debug; `IndexScope::{PinnedAndFavorites, AllText}`.
- Produces `EmbeddingCandidate { id: String, text: String, text_hash: String }`, `IndexedEmbedding { id: String, text_hash: String, vector: Vec<f32> }`, `IndexCursor { after_rowid: i64, disk_done: bool, pending_after_id: Option<String> }` with Default.
- Produces `IndexBatch { request_id: u64, profile: String, candidates: Vec<EmbeddingCandidate>, cursor: IndexCursor, done: bool }` and `SemanticSearchQuery { text: String, scope: Scope, request_id: u64, profile: String, vector: Vec<f32> }`.
- Produces `semantic::profile_id(&EmbeddingSettings) -> Result<String,String>`, `semantic::candidate(&StoredItem, IndexScope) -> Option<EmbeddingCandidate>`, `semantic::validate_vectors(&[Vec<f32>]) -> Result<usize,String>` and `semantic::cosine_distance(&[f32], &[f32]) -> Result<f32,String>`.
- Produces `embedding::parse_endpoint(&str) -> Result<claude::Endpoint,String>`, `embedding::prepare(&EmbeddingSettings, &str, &[String]) -> Result<claude::HttpRequest,String>`, `embedding::parse_response(u16, &[u8], usize) -> Result<Vec<Vec<f32>>,String>`.
- Produces `EmbeddingRequest { settings: EmbeddingSettings, inputs: Vec<String> }`, `EmbeddingCompletion { id: u64, result: Result<Vec<Vec<f32>>,String> }`, and `EmbeddingWorker::{start(runner, notifier),submit(request)->Result<u64,String>,cancel(),try_result()->Option<EmbeddingCompletion>}` following existing `AiWorker` lifecycle. No engine Command/Event variants in this task.

- [ ] Write portable tests before implementation. Response order fixture has indexes 1 then 0 and must return vectors in input order; repeated/missing indexes, mixed dimensions, zero and non-finite vectors, oversize and non-2xx must fail with sanitized errors. Address fixtures include root, `/v1`, complete path, proxy prefix, IPv6 and rejected userinfo/query/injection. A blocked runner test submits A, then B and C; only A and C may run, only C may publish; cancel/drop must not join A.

```rust
let reply = br#"{"data":[{"index":1,"embedding":[0,1]},{"index":0,"embedding":[1,0]}]}"#;
assert_eq!(embedding::parse_response(200, reply, 2).unwrap(), vec![vec![1.0,0.0],vec![0.0,1.0]]);
```

- [ ] Run `CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --test embedding --test embedding_jobs` and record red failures for missing behavior.
- [ ] Implement interfaces and bounded validation. Default endpoint `https://api.openai.com`, model empty, enabled false, ciphertext empty, scope fixed/favorites. Model profile hashes normalized address/model/input format version; candidate text excludes pins/use stats and is bounded before hashing. Preserve prior Settings JSON through serde defaults. Factor the existing strict endpoint parser only enough to select `messages` or `embeddings`; both protocols retain exact existing address behavior. Use literal expectations, no source-grep tests.
- [ ] Run the new tests and `cargo test --test claude --test palette --test persistence`; format and self-review. Commit only owned files and write report containing exact commands, pass/fail evidence, signatures and concerns.

### Task 2: sqlite-vec storage, engine indexing and hybrid retrieval

**Files:**
- Create: `src/semantic_store.rs`
- Modify: `Cargo.toml`, `Cargo.lock`, `src/repository.rs`, `src/engine.rs`, `src/model.rs` (Command/Event), `src/lib.rs`
- Test: `tests/semantic_search.rs`, existing `tests/persistence.rs`, `tests/ai_search.rs`

**Interfaces:**
- Consumes all Task 1 types and pure functions.
- Add `Command::PrepareSemanticIndex { request_id: u64, cursor: IndexCursor }`, `Command::StoreEmbeddings { request_id: u64, profile: String, items: Vec<IndexedEmbedding> }`, `Command::SemanticSearch(SemanticSearchQuery)`.
- Add `Event::SemanticBatch(IndexBatch)` and `Event::EmbeddingsStored { request_id: u64, accepted: usize }`; query results remain existing Event::Results.
- SQLite methods stay internal; expose CLI/engine integration through these commands. Preserve normal Command/Event behavior and all old search paths.

- [ ] Add engine integration tests first using a temporary real SQLite file and real engine. Seed text with no lexical overlap, configure enabled embedding settings, obtain candidate hashes with PrepareSemanticIndex, submit independently chosen `[1,0]`, query `[1,0]`, assert real ID appears before flush while a second DB connection has no persisted vector; shutdown/reopen must restore it. Additional cases cover filters before top-k, text changes while request in flight, deleted/originally cleaned rows, scope changes, metadata-only statistics, dimensions/profile isolation, bounded candidate pages and expired queries.

```rust
engine.send(Command::SemanticSearch(SemanticSearchQuery {
    text: "退款说明".into(), scope: Scope::Clipboard, request_id: 20,
    profile, vector: vec![1.0,0.0],
})).unwrap();
assert_eq!(wait_results(&engine,20)[0].id, saved_id);
```

- [ ] Record the red test failure before production implementation. Resolve and pin a published non alpha sqlite-vec Rust crate, record the exact native `vec_version()`; statically register its extension once before SQLite opens.
- [ ] Implement lazy derived schema with an active profile and actual dimension; use `vec0` cosine KNN and mapping rows back to true items. Keep large SQL/index responsibilities in semantic_store.rs. Candidate paging reads only required text/metadata and no representations. Revalidate batches atomically before changing memory; enforce 4 MiB budget, profile/dimension coherence, stale hash and changed eligibility. For mixed valid/stale entries skip obsolete entries and acknowledge only accepted ones; invalid vector batches must not partially modify state.
- [ ] Incorporate pending vectors in actor dirty/deadline/pressure/flush/shutdown. Persist completed vectors in the same dirty transaction after original item changes and retention; clear pending only on successful commit. Delete orphaned mappings and vec0 rows on source delete/retention/content invalidation. Metadata-only changes do not rewrite vector coordinates. Query DB only when profile/dimension match; filter scope/eligibility and pending/deleted obsolete data before nearest-neighbor limit, merge memory vectors and lexical results by actual ID, exact matches first, max 50. No query commits or network.
- [ ] Run new storage tests plus `cargo test --test persistence --test ai_search --test search`; fmt and targeted Clippy. Commit owned changes, report exact version, interface implementation, test results and migration/error behavior.

### Task 3: Windows client, configuration, query and indexing UI

**Files:**
- Create: `src/semantic_ui.rs` (portable session/gating state), `tests/semantic_ui.rs`, `tests/windows_embedding.rs`
- Modify: `src/platform/windows/ai_client.rs`, `src/platform/windows/ui.rs`, `src/platform/windows/mod.rs` if module needed, `src/palette.rs`, `src/lib.rs`, `tests/palette.rs`
- Update: `README.md`, `THIRD_PARTY_NOTICES.txt`

**Interfaces:**
- Consumes Task 1 worker/protocol/settings and Task 2 engine Commands/Events.
- Produces native `ai_client::embed(&EmbeddingSettings, &[String]) -> Result<Vec<Vec<f32>>,String>` and `ai_client::validate_embedding_settings(&EmbeddingSettings) -> Result<(),String>`, reusing DPAPI and an explicitly bounded exchange.
- New Function variants `EmbeddingSettings`, `SemanticSearch`, `UpdateSemanticIndex`, `CancelSemanticIndex`; extend Context only as needed and update all consumers. Only enabled functions appear; cancel-index only during an active index session. Configuration always searchable.

- [ ] Write portable session tests for explicit cancellation after network completion but before local search completion, query edits/scope/config changes/hide, stale indexing session/profile replies, source changes and sequential batch acknowledgement. Expected visible selection and result-cache state must clear immediately, not just discard HTTP completion. Write palette behavior tests for enablement/search and Windows local TCP/DPAPI fixtures before native implementation.

```rust
// Cancellation must advance the current local request token as well as cancel HTTP.
let old_request = state.request_id;
cancel_semantic_view(&mut session, &mut state, &mut saved, &mut ready);
assert!(state.request_id > old_request);
assert!(saved.is_empty());
assert!(!ready);
```

- [ ] Record red tests; implement separate semantic UI state with clear query-vs-index lifecycles. Native network uses existing WinHTTP RAII/TLS defaults and sanitized errors, generalized response-body budget only where needed to support 2 MiB. Existing Claude remains limited to 64 KiB. Decrypted credentials and sensitive request copies use the current erasure wrappers; settings validation sends no POST.
- [ ] Add owned native Embedding settings dialog with enable/address/model/key/clear-key/index-scope fields; blank password preserves existing, clear disables, invalid configuration shows owned Message and does not save. Explicitly explain outgoing selected text and saved metadata-only document indexing. Add enabled-only semantic button and settings menu index/cancel actions, no hotkeys. Route results using request/generation/profile/query/scope checks; adopted semantic query/cache are revoked on cancel/settings return. Index batches are requested in bounded pages; submit current candidates to network, turn replies into StoreEmbeddings only while session is current; acknowledge accepted batches before progressing; display progress/error without auto-retry. Exit/hide/edit cancellation cannot block storage exit, index canceled on exit/config change.
- [ ] Update README and licenses with real behavior, API format, scope, text limits, write policy and actual verification limits. Run `cargo test --test embedding --test embedding_jobs --test semantic_search --test semantic_ui --test palette`, Windows target compile of fixtures, shared and Windows Clippy with `-D warnings`, fmt. Commit owned changes and report commands/results and outstanding Windows/API manual acceptance.

## Final acceptance

- [ ] One whole-feature review, one consolidated fix wave if findings; preserve accepted scope and report remaining material limitations.
- [ ] Root run fresh full shared suite, fmt and both target Clippy after fixes; build Windows Release and verify archive/executable hashes and packaged docs. Avoid repeating unchanged passing heavy checks.
- [ ] Record actual sqlite-vec version, test totals, source commit and semantic/ordinary search benchmark boundaries in `docs/verification.md`; benchmark model latency only when a real configured provider is available.
- [ ] Preserve delivery ZIP and checksum sidecar; clean only this plan's temporary review workspace when complete.
