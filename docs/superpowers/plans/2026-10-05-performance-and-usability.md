# Performance and Usability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use subagent-driven-development to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** 完成用户批准的全部性能、交互和功能优化并交付更新的Windows便携包。

**Architecture:** 保留单SQLite actor，将可替换查询与可靠命令分开调度；把扫描和维护改为按实际变更工作。原生UI复用有界后台任务、便携状态门控及现有异步消息机制，统一智能入口并补齐常用内容管理。

**Tech Stack:** Rust2024、Win32、rusqlite0.40.2、sqlite-vec0.1.9。

**Spec:** `docs/superpowers/specs/2026-10-05-performance-and-usability.md`

## Global Constraints

- 在当前文件夹、codex/quick-panel分支工作；不要创建额外工作树、推送或发布。
- 历史约55秒开始、健康情况下60秒内提交；收藏配置立即保存、正常退出保存。查询、预览、索引状态和接口测试只读或RAM，不额外落盘。
- 本地普通搜索；AI/Embedding手动触发、API失败安静回退；保留真实存储错误提示。
- 不调用真实API或读取用户密钥。所有新异步队列有界，关闭不等待非持久化网络/路径任务，旧回执不得操作新目标。
- 只能一个实施agent写代码，不创建子agent。你并非独占代码库，不回退他人更改。
- Cargo串行；采用CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0，完整测试加--test-threads=1；不要运行大库benchmark或清理非本项目文件。
- 使用有意义的行为测试，先确认预期失败再实现；Windows-only只可声明编译检查，不能声称执行。

### Task 1: 搜索调度、增量维护和界面所需数据接口

**Files:** 修改src/engine.rs、repository.rs、search.rs、semantic_store.rs、semantic.rs、model.rs，以及对应tests；必要时新增小型query调度模块并导出lib。Windows ui.rs仅允许添加新事件的兼容match分支，实际交互由Task3接入。

**Interfaces:** 保持现有Command::Search/AiSearch/SemanticSearch及Event::Results外形可用。新增以下跨任务接口（可添加derive/serde默认，字段名保持）：

```rust
pub enum SmartSearchMode { Auto, Claude, Semantic } // Default=Auto，serde兼容
// Settings新增 pub smart_search: SmartSearchMode
pub struct ClipboardMetadataInput { pub id:String, pub title:String, pub alias:String, pub tags:Vec<String> }
// Command新增 EditClipboardMetadata(ClipboardMetadataInput)
pub struct SemanticIndexStatus { pub profile:String, pub indexed:usize, pub needs_update:bool }
// Command新增 SemanticStatus { request_id:u64 }
// Event新增 SemanticStatus { request_id:u64, status:SemanticIndexStatus }
```

**Required behavior:**

- 可替换的结果查询不堆积无界FIFO；可靠Capture/Save/Load/Flush/Shutdown仍按确定顺序处理。查询代次跨Search/AiSearch/SemanticSearch统一。到达新的可靠命令后，未执行查询可重新基于当前数据运行，不能越过Shutdown发陈旧结果。
- 长FTS碰撞扫描及语义路径扫描在页边界检查代次和到期保存/可靠命令，允许让出；不通过固定候选数量截断来漏掉原有旧条目。可采用查询工作状态和重启/恢复策略，但不能在持续输入或批量捕获时使持久化饿死。
- 纯向量事务不改变源扫描代次；原文由RAM进入磁盘、修改旧rowid内容、相关标题/alias/tags/pin资格变化仍正确失效。
- pin覆盖用一次求值的ID集合，避免对每个向量遍历完整JSON。维护仅新增历史/保留策略变化/到期维护需要；待删集合一次计算，同事务清原文/FTS/向量，显式删除定点清理，不能丢失老库初始化修复能力。
- SemanticStatus计算有效的当前profile索引数，并报告有符合范围的内容缺少有效向量。包含已接收RAM向量和pending/deleted覆盖，不创建表或提交。状态仅显式请求，不加全量每键扫描。
- EditClipboardMetadata仅允许已固定剪贴板，名称非空、别名标签合理有界，保存立即；保留原text/representations及身份，检索标题/别名/标签立即生效，更新派生索引资格；再次复制同内容保留用户名称。避免为元数据改名重写blob。
- 命中正文时在返回的ItemSummary.subtitle中生成有界单行片段并保留来源；纯内存加工，原subtitle/正文不变。保持窗口动作按id加载原始项，不把摘要写回。查询归一化结果复用。

- [ ] 添加真实行为回归：阻塞writer期间30次查询只得到最新有效结果且夹入保存不丢；FTS碰撞取消/到期提交；纯向量压力提交游标前进；pin过滤大量pending不改变top50资格；小设置提交不进行不必要维护；元数据改名重启/再复制保留格式；状态空/完整/脏/profile变化；CJK正文片段不落盘。
- [ ] 运行定点测试确认缺失行为，再按以上边界实现并复跑相关suite；在报告写明实际RED/GREEN命令及结果。
- [ ] 检查fmt和shared严格Clippy。提交本任务代码，报告接口和任何必要裁决，供下一任务使用。

### Task 2: 有界后台文件操作、预览与图片捕获

**Files:** src/platform/windows/ui.rs、actions.rs、clipboard.rs、mod.rs；src/clipboard_formats.rs；必要时新增src/preview.rs或src/background_job.rs等便携模块与tests；Windows测试及lib导出。不要修改Task1核心存储接口，除兼容新Event穷尽match的最小接线。

**Interfaces:** 使用Task1的现有Load/Results和新SemanticStatus事件，尚未做Task3UI时状态事件可存入App或安全忽略。新增便携后台任务与预览身份门控需向Task3报告。

**Required behavior:**

- 路径metadata/exists/is_dir与Shell打开/定位不得在面板UI线程同步执行。使用有界后台任务（单在途+最新预览、动作可靠但禁止重复提交），消息只携带owned值与代次；结束与关闭不join可能阻塞的路径任务。真正执行打开前核对动作是否仍有效；不要让旧查询/退出后的迟到操作打开别的目标。
- preview选择同id且内容版本未变时不重新Load或解码；列表仅数据改变时重建，保留选中项与顶部可见项。开关预览不重复发多次Load，旧结果不能覆盖新选择。当前预览在新结果到达前可保留但标明正在加载，动作指向始终是当前选中项。
- 图片用WIC按预览像素边界生成缩略图（最大边1024、BGRA输出不超过4MiB，保持比例不放大小图），保留源尺寸显示；16MiB旧输出限制不再拒绝可读取的4K PNG。极端尺寸/压缩炸弹仍有输入像素上限，并说明限制。
- 同时存在PNG和超预算DIB时保留有效PNG、跳过冗余DIB，不因其使整条采集失败；总保留载荷仍8MiB。只有DIB超限保持原限制。恢复PNG时按需提供CF_DIB兼容表示：设置源像素/临时内存上限（最大约64MiB解码输出），不得每次预览生成原图DIB或把派生DIB保存到数据库。保持文本/HTML/RTF/HDROP现有保留行为。
- 文本预览使用只读可滚动且能选中复制的原生Edit，搜索导航不截获预览中的方向键/Ctrl+C；图片预览仍按需GDI绘制。预览文本仍有6000字符预算并明确截断。
- 从外部重新唤起时全选旧query；打开预览、内部恢复查询不重置用户选区。保持IME与动作门控。

- [ ] 先用便携helper回归尺寸缩放、载荷格式选择、preview身份、后台任务有界和关闭不等待；native fixture覆盖PNG恢复结构与新代码编译。禁止source grep测试。
- [ ] 实现后跑相关shared测试与Windows all-targets严格Clippy（与控制器串行），明确Windows未执行边界。
- [ ] 提交代码，报告为Task3新增/改变的App字段、函数、事件处理以及测试证据。

### Task 3: 统一智能入口、索引状态与常用内容管理

**Files:** src/platform/windows/ui.rs、dialogs.rs、theme.rs；src/palette.rs、semantic_ui.rs、ui_state.rs；必要时新增src/smart_search.rs；对应tests、README.md。

**Interfaces:** 消费Task1的Settings.smart_search、SemanticStatus命令/事件、EditClipboardMetadata。复用Task2的预览与动作门控，不另建重复任务线程。

**Required behavior:**

- 主搜索栏只有一个“智能查找”按钮；没有启用任何能力时点击进入适用配置。常规设置新增首选能力Auto/Claude/Semantic，按自然语言解释；Auto在有效语义索引可用时优先语义，否则用已启用Claude，只有Embedding无索引时引导“建立索引”。保留两个独立API设置及测试连接。
- 现有按意思查找/语义查找功能搜索词映射到统一入口或明确指定策略的功能结果，不再让用户看到两个难区分的主按钮。正在请求时统一按钮可取消；切换query/scope/配置/隐藏处理沿用可靠撤销逻辑。
- 智能检索前异步取得SemanticStatus，0有效向量不得发送Embedding查询。明确显示未建立、已索引N、待更新状态及可点击/可搜索的建立更新入口；仅在启动/显示/内容变更后需要时刷新且合并，禁止每键COUNT，禁止自动上传建立索引。API故障仍安静本地回退，详情由设置测试按钮提供。
- 固定剪贴板右键提供编辑名称/别名/标签，三个原生字段，提交EditClipboardMetadata；不编辑正文或格式。切换/关闭/取消不会提交，不固定项可先固定。旧数据加载兼容。
- 结果行保留类别、来源/命中片段并增加本地保存时间，紧凑排版不新增全局快捷键；长标签不挤掉核心标题。缓存系统主题palette和画刷，随主题/DPI事件刷新，避免每行重读注册表和反复建相同画刷。
- 新交互继续支持键盘、IME、浅深主题、多屏DPI；文本预览的局部复制由Task2原生控件完成。

- [ ] 先添加策略选择、空索引不调Embedding、preflight过期失效、元数据表单取消/提交、结果片段/时间与菜单目标保持的便携测试。
- [ ] 接入原生控件，跑相关shared tests与Windows严格Clippy；更新README操作与图片格式/预算说明。
- [ ] 提交并报告所有可见变化和测试证据。

## Final verification and delivery

- 全套共享测试串行、fmt、git diff --check、macOS与Windows all-targets Clippy；Windows Release构建、core-check。只对修改点做新增有意义的工作量/延迟测量，不重复无关大型benchmark。
- 整分支独立审查，处理确定问题；更新docs/verification.md，如实区分共享测试、交叉编译与Windows未执行。
- 在dist重新生成Windows x64 ZIP，核对EXE/文档字节、哈希、ZIP完整性、文档链接与工作树状态。
