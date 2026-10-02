# Windows Quick Panel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use subagent-driven-development or executing-plans to implement this plan task-by-task. Track execution below.

**Goal:** Build a native Windows clipboard and quick-access tool with immediate in-memory retrieval, low-frequency persistence and safe exit.

**Architecture:** A single background engine owns SQLite and the pending overlay. Win32 handles the tray, native text input, result list and clipboard events; background events wake the UI through a posted window message. Shared logic runs and is tested on macOS.

**Tech Stack:** Rust 2024, windows 0.62.2, rusqlite 0.40.2 with bundled SQLite, serde, sha2, pinyin.

**Spec:** ../specs/2026-10-02-windows-quick-panel-design.md

## Global Constraints

- Windows is the primary platform; use native Win32 controls and no embedded browser.
- History saves within the first dirty item's 60-second window under healthy storage. Continued copying does not reset the deadline.
- Favorites and confirmed configuration changes save immediately.
- No periodic writes without dirty data or required bounded WAL maintenance.
- Pending content budget is 16 MiB, item limit 8 MiB, decoded preview budget 16 MiB.
- Use short WAL transactions and synchronous=FULL; clear pending state only after successful commits.
- Exit flushes every accepted durable item before the process ends; errors keep the application alive.
- Use the shared model in src/model.rs. Changes to interfaces must be coordinated by the root agent.

## File ownership and interfaces

| Task | Owned files | Shared boundary |
|---|---|---|
| 1 | Cargo.toml, src/model.rs, src/search.rs, src/lib.rs, tests/search.rs | Search helpers consumed by the engine |
| 2 | src/engine.rs, src/repository.rs, tests/persistence.rs | Engine::start, send, try_recv; Command and Event model |
| 3 | src/platform/windows/clipboard.rs, actions.rs, src/clipboard_formats.rs, tests/clipboard_formats.rs | Native capture, restore, paste and open actions |
| 4 | src/platform/windows/mod.rs, ui.rs, dialogs.rs, theme.rs, src/ui_state.rs, tests/ui_state.rs | Native run function and shared engine events |
| 5 | src/main.rs, scripts, .github/workflows, README.md, LICENSE, packaging and integration fixes | A Windows executable plus reproducible checks |

## Task 1 Search and foundation

Consume StoredItem, ItemSummary and Scope from model.rs. Produce search::index_text(&StoredItem) -> String, query_expression(&str) -> Option<String>, matches(&StoredItem, &str) -> bool and rank(&ItemSummary, &str) -> i64.

- [ ] Add failing behavioral tests for Chinese one/two-character retrieval, pinyin initials, alias precedence and safely quoted FTS queries. A representative case:

    let item = fixture("周报模板", "zb", "C:/Documents/weekly.docx");
    assert!(search::matches(&item, "zhoubao"));
    assert!(search::matches(&item, "报"));
    assert!(search::rank(&item.summary, "zb") > search::rank(&other.summary, "zb"));

- [ ] Run cargo test --test search and observe missing search behavior.
- [ ] Implement normalized word, CJK unigram/bigram and pinyin indexing. Use parameterized MATCH expressions; rank exact aliases and titles above fuzzy matches.
- [ ] Re-run the search tests, then commit only the owned files.

## Task 2 Persistence engine

Produce Engine::start(EngineOptions, Arc<dyn Fn() + Send + Sync>) -> Result<Engine, String>, Engine::send(Command) -> Result<(), String>, Engine::try_recv() -> Result<Event, TryRecvError>. The notifier executes after an Event is queued. Engine owns one worker and serializes every write.

- [ ] Write tests against a real temporary SQLite file. Before the implementation, run cargo test --test persistence and verify the missing engine fails.
- [ ] Test that capture is searchable before disk persistence:

    engine.send(Command::Capture(snapshot("尚未保存"))).unwrap();
    engine.send(Command::Search(SearchQuery { text: "尚未", scope: Scope::All, request_id: 1 })).unwrap();
    assert_eq!(wait_results(&engine, 1)[0].title, "尚未保存");
    assert_eq!(read_disk_item_count(&database), 0);

- [ ] Test final flush by requesting Shutdown, waiting for ExitReady, reopening the SQLite file and reading the text and binary representations. Test failed commits by using SQLite read-only/query-only fault control or a real write lock; pending records must remain retrievable and retry without duplication.
- [ ] Implement schema, FTS, BLOB representations, settings, persisted favorites and pending overlay. Stable content hashes deduplicate clipboard entries while preserving format distinctions.
- [ ] Implement first-dirty deadline, byte pressure, merged usage statistics, bounded retention and idle WAL maintenance. recv_timeout waits until actual work/deadline; idle loops do not write.
- [ ] Test that a later capture does not move the first dirty deadline, that favorites/settings survive immediate reopening, that deletes hide pending and saved results, and that duplicate retries do not multiply usage.
- [ ] Re-run persistence tests and commit only owned files.

## Task 3 Windows clipboard and actions

Consume model.rs. Produce clipboard::capture(HWND, usize) -> Result<Option<ClipboardSnapshot>, String>, clipboard::restore(HWND, &StoredItem, bool) -> Result<(), String>, clipboard::paste_to(HWND) -> Result<(), String>; actions::open_target(&StoredItem) and reveal_target(&StoredItem) -> Result<(), String>. Expose clipboard::sequence_number() -> u32 for self-write suppression.

- [ ] Add shared pure format tests before implementing UTF-16 text and CF_HDROP parsing, including malformed buffers, multi-file Unicode paths and oversize representations. Run cargo test --test clipboard_formats to observe failures.
- [ ] Implement owned snapshots for CF_UNICODETEXT, CF_DIB/CF_DIBV5, CF_HDROP, PNG, HTML Format and Rich Text Format. Only recognized transferable HGLOBAL formats may be restored. Ignore agreed sensitive/transient markers and application exclusions.
- [ ] Use bounded OpenClipboard retries and RAII closing; do not retain foreign clipboard pointers. Restore formats without leaking GlobalAlloc handles or copying on a failed ownership transfer.
- [ ] Verify foreground target before Ctrl+V and respect UIPI. Use ShellExecute for document/folder opening and Shell item APIs for reveal. Report real errors.
- [ ] Run format tests on macOS and cargo check --target x86_64-pc-windows-gnu for Windows-specific code; commit owned files.

## Task 4 Native panel and lifecycle

Consume engine and native actions. Produce platform::windows::run() -> Result<(), String>.

- [ ] Add shared UI-state tests that protect selection during result refresh, prevent Enter/Esc actions during IME composition, and distinguish Hide from Exit. Run cargo test --test ui_state to observe missing behavior.
- [ ] Implement tray, configurable registered hotkeys, per-monitor DPI, native Edit input, virtual or bounded result list, Chinese labels and scoped search.
- [ ] Use the approved compact layout with system/light/dark themes, Segoe UI, calm blue focus accents, clear selection, action footer and an optional preview. Search has native IME handling; background updates preserve selected identity.
- [ ] Add drag/drop and native picker entry points; favorites can edit title, alias, tags and open-with application. Provide context actions, missing-target relink and settings controls for themes, hotkeys, pause and application exclusion.
- [ ] UI handling never waits for normal SQLite queries. Background results notify WM_APP; dismissing the panel does not stop capture.
- [ ] Exit closes admission, drains accepted capture/parse jobs, asks engine for final save, then destroys resources only on ExitReady. A failed exit offers retry/cancel and keeps buffered data. System shutdown uses its own bounded path.
- [ ] Run shared UI tests and cross-target checks; commit owned files.

## Task 5 Integration and delivery

- [ ] Replace the bootstrap entry point with the Windows GUI and a non-Windows core-check/benchmark CLI.
- [ ] Add a Windows build script, GNU cross-build script, portable ZIP packaging and Windows CI tests/build; no publishing or pushing.
- [ ] Add a release-mode search benchmark against a real database with mixed Chinese/path fixtures. Report measured core numbers as local core evidence, not Windows end-to-end latency.
- [ ] Run cargo fmt --check, cargo test, cargo clippy --all-targets -- -D warnings, Windows cargo check/test compilation and the Windows release cross-build.
- [ ] Inspect imported DLLs, package the executable with README/license, and complete independent review of storage, native lifecycle and whole-branch behavior.
- [ ] Update README with actual commands, shortcuts, persistence behavior, data location, verification evidence and Windows runtime checks that still need a real machine. Commit the reviewed first version.
