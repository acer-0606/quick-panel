# External Paths and Screenshots Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 主要从本工具之外的资源管理器访问自动推荐常用路径，并提供内存区域截图到 PNG 剪贴板。

**Architecture:** 外部观察与截图都复用原有引擎保存、原生菜单、后台任务和取消机制；独立路径状态与独立 entry 保护人工收藏和默认索引边界。先完成可验证的引擎数据契约，再接 Windows 采集/界面，最后接截图；实施串行避免共享接口冲突。

**Tech Stack:** Rust 2024、Win32/Shell/WIC、rusqlite 0.40.2、sqlite-vec 0.1.9；不引入常驻 WebView、模型或系统驱动。

**Spec:** `docs/superpowers/specs/2026-10-05-external-paths-and-screenshots-design.md`

## Global Constraints

- 路径统计复用历史约 55 秒开始、正常 I/O 下 60 秒内的事务窗口，持续访问不能延期；仅提交变化行。
- 收藏、配置、明确“不再推荐”及清空立即保存。空闲无变更不写盘，查询不提交。
- 自动路径不加入 `items`/FTS/向量表，不自动调用 AI；新旧安装默认关闭此可选采集功能。
- 最多 1024 个有效路径、128 个抑制项、2048 UTF-16 单元每条路径、2 MiB 总路径缓存；观察批次最多 32 项且有独立有界队列预算。
- 探测周期 2 秒、稳定 1.5 秒、跨源去重 30 秒、排名半衰期 7 天、自动记录保留 90 天；Explorer 权重 1，Recent 权重 0.5。
- Recent 每轮至多 16 个变化链接，基线最多 2048 个；无 Resolve/目标打开/目标网络探测。
- 空查询至多 5 个自动推荐，有输入至多 10 个；总列表最多 50 个。
- 截图原始像素预算 64 MiB，PNG 交付预算 8 MiB；默认无截图全局热键，不生成临时图片文件。
- 保持现有数据、快捷键、低频保存、退出与 API 安静回退兼容；存储失败仍可见。
- Cargo 串行，测试不接触真实用户路径/剪贴板/屏幕/密钥；Windows 运行未验证需明确记录。

---

### Task 1: 外部路径统计与低频保存核心

**Files:**
- Create: `src/path_activity.rs`, `src/path_activity_store.rs`, `tests/path_activity.rs`
- Modify: `src/model.rs`, `src/lib.rs`, `src/engine.rs`, `src/repository.rs`
- Test: `tests/persistence.rs`, `tests/semantic_store.rs`（仅必要接口/隔离回归）
- Compatibility only: `src/platform/windows/ui.rs` 新事件的最小 match 分支，Task 2 再接可见交互。

**Interfaces:**

```rust
// src/path_activity.rs, public, serde-compatible where persisted
pub struct PathActivitySettings {
    pub enabled: bool,             // default false
    pub recent_documents: bool,    // default true; gated by enabled
    pub excluded_paths: Vec<String>,
}
pub enum ObservationSource { Explorer, RecentDocuments, QuickPanel }
pub struct PathObservation {
    pub path: String,
    pub kind: crate::model::ItemKind,
    pub observed_at_ms: i64,
    pub source: ObservationSource,
}
pub struct PathSuggestion {
    pub key: String, pub path: String, pub title: String,
    pub kind: crate::model::ItemKind,
    pub visits: u64, pub last_seen_ms: i64,
}
pub fn normalize_path(path: &str) -> Result<String, String>;
// model::Settings adds path_activity: PathActivitySettings with serde default.
// Command additions:
// ObservePaths(Vec<PathObservation>), SuppressPath(String), ClearPathActivity
// Event additions:
// PathSuggestions { request_id: u64, items: Vec<PathSuggestion> }, PathActivityChanged
// SaveStats adds semantic_status_changed: bool so path-only commits need not rescan vectors.
```

后台实现可自行设计私有 policy/store 函数，但上述公开契约与默认值固定。`PathSuggestions` 随当前三类结果查询产生，复用其编号与 latest-only 检查，不新增会互相覆盖 Search 的查询命令。新增事件没有消费者时保持当前 UI 行为。

- [ ] **Step 1: 先写可验证的 RED 回归**

```rust
#[test]
fn absolute_windows_path_is_normalized_without_target_io() {
    assert_eq!(normalize_path(r"C:\\Work\\Project\\").unwrap(), r"c:\work\project");
    assert!(normalize_path(r"relative\file.txt").is_err());
    assert!(normalize_path(r"C:\secret.txt:stream").is_err());
}
```

其余行为测试使用公开 Engine/channel 和真实临时 SQLite：启动时启用设置并先保存；发送两个同路径不同来源的30秒内观察；搜索收到一条候选且 visits=1；独立连接查询发现尚无路径持久数据；Shutdown 后重开能查到候选。发送不同路径的持续观察，使用缩短的 `EngineOptions.save_interval_ms`，证明首次截止期不被延期。占用真实 SQLite 写锁制造保存失败，释放后 Flush 能恢复；失败期间仍可检索候选。验证关闭/排除/抑制/清空及恢复设置、0 items/FTS/vec 新行、旧配置反序列化默认关闭。

Run: `cargo test --locked --test path_activity -- --test-threads=1`；确认首次是预期缺少契约/行为失败。

- [ ] **Step 2: 实现独立缓存与同事务增量提交**

```rust
// Repository owns the path cache on the sole engine thread.
// Existing dirty predicate must include path changes; mark_dirty only on first change.
// Existing Repository::commit writes changed path rows in the SAME transaction.
// Only after tx.commit succeeds clear path dirty/deleted sets.
// No path-only mutation increments source_generation or changes vector inputs.
let semantic_status_changed = !pending.is_empty()
    || !deleted.is_empty() || settings_dirty || !vectors.is_empty();
```

上段是行为伪码，变量绑定以现有类型 API 为准，不能添加恒假的测试。缓存路径长度/内存/数量与观察队列均有界，允许拒绝无效或超预算的新观察但不得清掉已接受历史。路径语法包含盘符/UNC、斜杠、重复分隔符、点段、根目录；不做真实目标访问。清空/抑制与设置在同保存机制立即确认，失败不丢状态；无变更读操作不创建表。

- [ ] **Step 3: 验证全部策略边界与兼容**

测试精确目录组件排除、大小写去重、回拨/超前时间、同路径去重、半衰期排序、稳定同分、全部范围/中文拼音、隐藏及字节/数量界限、压力与退出。三类查询产生候选前后都遵守当前取消/截止期；候选最多读内存，不执行文件 I/O，不扫描正文。开启统计后发出的自动路径事件不能被既有语义索引遍历到。

Run focused path/persistence/semantic tests, `cargo fmt --check`, macOS strict Clippy。只在本任务收尾运行一次全共享测试以建立集成基线，Windows 新match由Task2复检。

- [ ] **Step 4: Commit and report**

提交本任务范围，报告公共接口、RED/GREEN证据、通过数、限制和任何设计疑问。不要创建子 agent，不代替 controller 做独立审查。

### Task 2: Windows 外部采集和常用推荐交互

**Files:**
- Create: `src/platform/windows/path_monitor.rs`, `src/path_observer.rs`, `tests/path_observer.rs`
- Modify: `src/platform/windows/mod.rs`, `src/platform/windows/ui.rs`, `src/platform/windows/dialogs.rs`, `src/palette.rs`, `src/ui_state.rs`, `Cargo.toml`, `README.md`
- Test: `tests/palette.rs`, `tests/ui_state.rs` and new Windows fixtures if meaningful.

**Interfaces:**
- Consumes exactly Task1 `Settings.path_activity`, `PathObservation`, `PathSuggestion`, commands/events and `SaveStats.semantic_status_changed`.
- Produces `palette::Entry::Suggested(PathSuggestion)` with `suggested()` accessor; keep `saved()` limited to persisted items. Suggested ID is deterministic `path-suggestion:<key-hash>`; no fake Load/Pin/Delete requests.
- `PathObserver` portable policy accepts foreground identity, path observations and monotonic times/config generation; native Shell data remains outside core policy.

- [ ] **Step 1: RED cancellation/visit policy tests**

```rust
// Behaviour to cover with public policy APIs defined in the new module:
// observe(window=10,path=A,t=0) -> none
// observe(window=10,path=A,t=1500) -> A once
// observe(window=10,path=A,t=4000) -> none
// lose_foreground(); regain_same_window_and_path() -> no new visit
// navigate(window=10,path=B), stable -> B
// disable or change config while a probe is in flight -> ignore old receipt
// repeated tick with an occupied native worker -> no second job
```

测试用受控观察值/线程，不读取实际 Shell。另测 new-window path、快速跳转、同路径多来源、Windows11隐藏视图被拒绝的selection policy（其真实原生识别仍需Windows验收）。

- [ ] **Step 2: 接 Windows 背景观察**

```rust
// Two-second timer only while enabled; Win32 foreground identity check is cheap.
// Matching visible Explorer root -> submit one background Shell probe if idle.
// IShellWindows + IServiceProvider/IShellBrowser active visible view -> folder PIDL/path.
// Never use a window title as a filesystem path.
// Recent: directory-change signal -> <=16 changed .lnk parses; baseline <=2048.
// No Resolve(), target metadata(), or network existence checks in this monitor.
```

COM apartment/handles have RAII lifetime. Active tab view visibility and root/foreground checks must select only current Explorer view; ambiguous views skipped. One worker receipt must release occupied state on success/failure; disabling/exit removes timer/notifier and invalidates old generations, never joins blocking Shell. Recent initial baseline records metadata only, incremental changes alone produce observations; apply configuration/exclusions in both producer and engine. Errors are optional-feature status, not repeated search-page modal warnings.

- [ ] **Step 3: 接统一结果与动作、设置**

```rust
match entry {
    Entry::Suggested(path) => {
        // Synthesize an action-only StoredItem with target=path.path.
        // Reuse existing target epoch/ActionTicket and bounded native worker.
        // Explicit Favorite invokes existing editor; Suppress sends SuppressPath(key).
    }
    _ => { /* preserve existing branches */ }
}
```

此处片段描述接线位置，不允许用空分支替换现有逻辑。空查询至多5项、有输入至多10项；保留已固定/匹配的人工条目优先及总50项。用规范路径与已有收藏去重；自动项标明观察来源性质，时间使用最近访问，不能写成保存时间。针对事件编号同时管理 saved/suggested batches，迟到路径不能回灌新查询。后台同身份刷新保持选择，目标消失或模态切换仍撤销。路径变化无需触发语义扫描，path-only Saved 使用新flag跳过状态失效。

设置入口可由功能搜索及设置菜单打开，含启用、Recent、排除目录、清空统计。排除输入每行一条，校验最多128条及规范语法。关闭功能停止工作且隐藏自动项。新增项目可通过键盘完成，不新增默认全局热键。README 说明主来源为外部资源管理器、标签页/第三方覆盖边界、可选开关、本地排名和低频写盘。

- [ ] **Step 4: Verify and commit**

Run focused observer/palette/ui tests, macOS+Windows all-target strict Clippy, fmt/diff checks。Windows fixtures只交叉编译不可计入执行数；实现报告列出Shell/Recent调用与避免目标I/O的证据，任何无法可靠识别的分支必须跳过而非猜测。完成后提交并交回controller审查。

### Task 3: 原生区域截图到 PNG 剪贴板

**Files:**
- Create: `src/screenshot.rs`, `src/platform/windows/screenshot.rs`, `tests/screenshot.rs`
- Modify: `src/platform/windows/mod.rs`, `src/platform/windows/images.rs`, `src/platform/windows/clipboard.rs`, `src/platform/windows/ui.rs`, `src/platform/windows/dialogs.rs`, `src/model.rs`, `src/palette.rs`, `src/hotkeys.rs`, `README.md`
- Test: `tests/hotkeys.rs`, `tests/palette.rs` and native codec fixtures.

**Interfaces:**
- Adds `Function::Screenshot`; settings adds screenshot hotkey modifiers/key default0/0, with serde backward compatibility.
- Reuses existing `Representation { format: "PNG", data }`, capture engine and bounded compatibility DIB preparation; no JPEG-only clipboard type or new screenshot DB.
- Portable screenshot geometry/gate policy owns selection bounds/budgets/operation generation and expected clipboard sequence; native overlay/codec are separate.

- [ ] **Step 1: RED geometry and lifecycle tests**

```rust
// normalize drag (-100,50)->(-500,300) within virtual desktop -> width400,height250
// zero-width/height -> cancellation; cross-monitor clipping is checked in i64
// 4096x4096x4 == 64MiB accepted; overflowing/over-budget rectangle rejected
// cancel or exit before encoding receipt -> no clipboard write
// clipboard sequence changes while encoding -> no replacement of newer content
// task remains occupied until actual receipt even when cancelled; failure permits retry
```

使用纯policy与真实受控线程；不得测试实际抓屏或用户剪贴板。

- [ ] **Step 2: 实现选区、内存捕获、WIC PNG编码**

```rust
// Explicit command hides panel, creates per-monitor-DPI-aware topmost selection overlay.
// Drag uses GetCursorPos screen coordinates, including negative virtual origin.
// Esc/right click/display change -> cancel and free all owned GDI/window resources.
// Confirmation hides overlay before DwmFlush/BitBlt into a <=64MiB selected DIB.
// Background WIC encoder writes PNG to memory; enforce <=8MiB output.
// No temp files; no model calls; no continuous screen capture or idle screenshot thread work.
```

选区不要求缓存整个虚拟桌面；只抓确认区域，资源按所有失败分支释放。UI跨原生调用不得持有App可重入借用。屏幕内容变化或受保护窗口可影响结果，在文档记录实机边界。

- [ ] **Step 3: 安全写剪贴板与历史、可选快捷键**

```rust
// Before clipboard mutation: validate task current + unchanged clipboard sequence.
// Prepare allocations first, open clipboard, recheck sequence, then EmptyClipboard/SetClipboardData.
// Put PNG plus bounded DIB compatibility; no fake file path / CF_HDROP temporary file.
// Suppress this exact clipboard update and submit a single explicit Capture only if recording allows it.
```

占用/编码/内存/预算失败保持旧剪贴板并给一次明确反馈。剪贴板写入成功而历史入队失败要区别提示，不假称图片未复制；退出时停止新截图并保存已接收截图，不等待编码网络或Shell任务。暂停记录仍允许截图复制，但不进入历史；排除应用规则保持有效。快捷键扩展保留既有两组交换/失败回滚逻辑，第三组默认关闭且不能破坏旧绑定。功能搜索和菜单提供截图入口，取消后恢复合理焦点，不强制唤起面板抢走目标应用。

- [ ] **Step 4: Complete verification and commit**

Run focused screenshot/hotkey/clipboard tests, full shared suite once on final portable source, macOS+Windows all-target strict Clippy and fmt/diff checks。原生编码fixture可编译验证，零Windows执行须明确。README说明PNG剪贴板、可选快捷键、记录暂停与预算。提交并报告后controller做最终范围审查、更新验证记录与Windows ZIP。
