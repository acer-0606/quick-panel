# Usability and Performance Repair Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** 修复2026-10-12审查的全部确认问题和明确优化，使原生UI、流程及低写入行为可靠，并取得真实Windows CI绿色交付。

**Architecture:** 保留现有Win32 UI、共享状态模型、单SQLite actor、单工作槽。对原生消息消费者做最小抽取以可执行测试，跨层状态只保留一份权威实现；UI适配使用一致的客户区/工作区几何与实际滚动。

**Tech Stack:** Rust1.95.0 / windows0.62 / rusqlite bundled SQLite / Win32 / Node24.19.0 / Pi。

**Spec:** docs/superpowers/specs/2026-10-12-usability-performance-repair-design.md

## Global Constraints

- 与用户交互和产品文案使用中文。Windows10/11 x64、原生Win32，保留现有主题、IME、快捷键与默认禁用的可选能力。
- 普通历史最多60秒缓冲，默认提前5秒开始保存；收藏、固定和配置等明确修改立即保存；正常退出排空已经接纳的持久化。不能用更频繁写盘来替代内存优化。
- 账号填入保留窗口实例、PID、进程创建时间、根身份、epoch/generation和逐次fresh sample校验；不盲发Tab/Enter、不自动提交登录、不放宽权限。全窗采集256节点/8层/每命令2秒，祖先核验64节点。局部完整采集仍仅本次填入。
- 固定Rust1.95.0；Cargo串行：CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0。macOS QP_TEST_NODE=/Users/chuwujie/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin/node。
- 仅合成数据，无真实API/凭据/剪贴板/用户DB；不新增依赖或迁移。一个实现者/一个Cargo；root只docs/scratch/GitHub协调；禁止子代理再派子代理。
- 原目录codex/quick-panel；不建worktree、不force、不覆写旧release/tag；由root操作GitHub验证和最终发布。
- 真实行为RED→GREEN；编译错误不是行为RED，未执行的原生fixture不能算通过。每次命令记录argv/env/exit/duration/raw log。不可用镜像模型或源码字符串断言替代实际生产行为。

---

### Task 1: UIA超时与Windows原生验证恢复

**Files:** src/account_fill_execution.rs; src/platform/windows/account_target.rs; account_picker.rs; account_ui.rs（仅原生fixture及确证CI根因）; tests/account_fill_execution.rs; tests/windows_account_fill.rs; .github/workflows/windows.yml; rust-toolchain.toml（如需要统一版本）。

**Interfaces:** 消费现有FillError、Automation::query、Runtime教学票据/控制器和VaultController::state/poll。生产错误分类的新增小函数由实现者在报告写准确签名；其必须真实用于query。保留外部填入协议/数据库不变。向后续任务交付可靠的原生夹具、固定工具链、明确CI失败诊断和当前接口。

- [ ] 读取target/review-2026-10-12/ci-and-uia.md及两个失败log；逐个定位三项失败，不把fixture错误当产品丢数据。把断言补为包含脱敏实际状态/结果/窗口坐标，足以诊断Win32命中与采样错误。
- [ ] 写/执行实际生产分类回归：provider timeout在截止前仍有准确超时分类；失败调用耗尽deadline可触发BudgetExceeded；普通不支持错误不可变成功；成功但迟于deadline仍拒绝。如需抽取，先保持旧行为获得有效RED，再最小修复；例如期望断言为`assert_eq!(classify_query_failure(true, false), FillError::BudgetExceeded);`，该函数定义为`pub fn classify_query_failure(provider_timeout: bool, deadline_expired: bool) -> FillError`并由真实query调用，或采用同等真实类型保留更准确分类。
- [ ] 修正Preparing fixture，用`assert_eq!(runtime.flow.vault.state(), SaveState::Preparing)`/`assert!(!can_shutdown())`验证已接纳，然后受控pump至Prepared、保留回执再验证候选；既测准备中筛选也测写入中筛选。不得删除保留保存及不续填断言。
- [ ] 修复picker/270节点失败的实际原因。可注册真实合成窗口类并按窗口客户区转换真实点坐标、泵消息建立稳定场景；保留生产回调/错误处理。不能扩大既定采样预算或绕过目标检查作为测试修复。未知CI结果先报告NEEDS_CONTEXT及可提交诊断，由root运行验证分支。
- [ ] 固定CI与host工具链1.95.0，保持串行测试和所有原生fixture；本地跑`cargo test --locked --test account_fill --test account_fill_execution --test account_fill_ui --test account_vault -- --test-threads=1`，host/Windows all-target check和严格Clippy、fmt/diff。有新具体问题才重复相关部分。
- [ ] 提交owned文件，报告给root，由root push验证分支取得真实Windows结果。继续处理本任务相关失败直至门禁明确；CI全套在Task4最终必需绿色，外域失败要把准确证据交接而不是跳过。

### Task 2: 原生界面适配与用户操作一致性

**Files:** src/platform/windows/dialogs.rs; organizer_ui.rs; harness_memory_ui.rs; account_ui.rs; ui.rs; src/settings_ui.rs; src/organizer_ui.rs; src/smart_search.rs; src/ui_state.rs; tests/settings_ui.rs; tests/organizer_ui.rs; tests/smart_search.rs; tests/ui_state.rs；必要的小共享布局模块与lib导出。

**Interfaces:** 消费Task1后的Runtime/fixture，保留账号安全状态；消费现有Navigation、Memory管理状态和SmartAction。新增几何/交互helper必须是生产consumer的单一实现并在报告列签名。Task4将在ui.rs的调度/入口上集成，报告交代导航返回与布局API。

- [ ] 逐条读取review的F1/F2/F3/UI1–UI5。编写能够驱动真实模型/原生consumer的RED：人工记忆A不改字段保存、回执前改选B、回执后Save/Delete仍不能错对象；按钮Tab焦点揭示且聚焦不触发测试；显式Claude→Semantic和Semantic→Claude继续指定能力；主按钮仍可取消。
- [ ] 修复记忆列表/编辑ID同步及动作时核对，确认框带摘要。修复显式功能与toggle入口区分。修复建议→记忆→设置的导航栈和草稿来源，不用嵌套bool丢失Memory frame。
- [ ] 叶子焦点揭示复用WM_SETFOCUS子类思路，不能单补BS_NOTIFY导致WM_COMMAND聚焦执行。相同测试结果文本跳过SetWindowText，确保长结果选区/滚动不因100ms刷新消失。
- [ ] 将建议页、账号页、主面板几何放入实际使用的有界布局；回归输入包含客户区h=180与260@96dpi，以及100/150/200/250%和窄工作区。必须验证所有动作非负/可达，相邻可见控件不相交；空间不足使用真实滚动内容，不能max(1)伪通过。最小尺寸由客户区需求转外框并裁工作区。可抽取通用矩形断言`a.x+a.width<=b.x || b.x+b.width<=a.x || a.y+a.height<=b.y || b.y+b.height<=a.y`用于真实返回几何，不用手写镜像布局。
- [ ] 本轮触及窗口的DPI尺寸裁到目标工作区；主面板把用户逻辑尺寸保存在RAM并在呼出时裁剪，不写配置。维持IME处理和原生控件主题；不要重设计全部视觉风格。
- [ ] 执行相关共享回归及新合成Windows消息/控件fixture，host/Windows strict/check、fmt/diff；提交owned文件，写完整报告。仅根代理运行新的远端Windows验证；不得把只编译的fixture称为通过。

### Task 3: 正文读取、语义资格与写入维护性能

**Files:** src/engine.rs; repository.rs; search.rs; semantic.rs; semantic_store.rs; account_vault_store.rs；tests/search.rs; persistence.rs; semantic_search.rs; account_vault.rs，及现有模块内部行为/资源probe。

**Interfaces:** 消费现有Repository/Actor、pending overlays、IndexScope、查询generation及结果Summary；保持外部事件协议稳定，若必须改内部候选则报告准确类型与consumer。Task4重试调度将消费Actor现有命令接口，不在本任务加入新的网络执行路径。

- [ ] 用合成实际SQLite与生产Actor建立有效RED：固定范围中大量未固定长文不应被全文物化；只Used/重复Capture而派生输入相同时状态不得重扫；metadata命中Top50的片段工作应有界且不二次全正文normalize；只有vault事务时到期/压力checkpoint仍被安排。保存raw计数/字节/耗时，不用臆测物理IO。
- [ ] SQL/metadata先过滤语义范围，正确合并pending pin/unpin/content/delete；整轮profile读取复用。真正资格/内容/profile变化仍失效，支持未保存新固定，不让优化漏索引。
- [ ] 搜索复用正文确认的匹配片段或位置；metadata命中用有界正文预览。保留Unicode lowercase展开、中文/拼音/尾部子串和原排名。显示120字符不能以全Top50正文拷贝为常规代价；不增持久化片段缓存。
- [ ] 所有实际写事务复用既有WAL维护安排，保留5分钟/16MiB/64MiB策略，vault立即耐久保存；幂等重放不制造多余维护，不顺便flush普通history。回归覆盖先完成普通checkpoint，再只有vault写入及正常退出。
- [ ] pending历史容量改为增量计数，涵盖insert/replace/dedupe/delete及flush失败重试，增加明确条目/metadata headroom保持有限。真实断言按合成相同payload不同条目数比较累计扫描/计数字段；不声称16MiB等于RSS，不因查询触发写盘。
- [ ] 跑覆盖到的search/persistence/semantic/account存储回归、host/Windows strict/check、fmt/diff，报告行为RED/GREEN及测量前提。无需重复Node或不相关GUI套件；提交owned文件。

### Task 4: 空闲唤醒、明确重试与完整验证交付

**Files:** src/platform/windows/ui.rs; account_ui.rs; src/account_fill_ui.rs; organizer_ui.rs; engine.rs（仅调度/重试的必要更改）; palette.rs; settings_ui.rs; tests/account_fill_ui.rs; organizer_coordinator.rs; organizer_ui.rs; palette.rs; settings_ui.rs; README.md; docs/verification.md; Cargo.toml/Cargo.lock（版本）；现有构建打包脚本仅必要变化。

**Interfaces:** 消费前三任务的窗口/导航/Actor状态与测试。复用既有OrganizerRetry/Tick、SemanticIndex/Cancel、hint事件及实际crypto worker状态，报告新增唤醒/调度接口的准确签名。

- [ ] 写有效回归：关闭且锁定无任务不需要100ms定时器；保存/回执/退出仍能完成；空闲锁到期仍执行且新的操作可唤醒；显式重试不等3600秒周期，忙时合并调度且关闭能力不发API；空查询50普通历史+有效路径仍有最多5路径入口，总行数<=50，固定/收藏不被挤走，非空查询保持相关性。
- [ ] 按真实工作/截止时间控制账号timer，必要事件唤醒而非无条件常驻100ms。确保创建/解锁、隐藏面板后保存、失败重试、锁屏、正常退出、hint版本失效都不丢事件或放宽校验。不要在每次超时新建线程。
- [ ] 明确重试触发一次受现有权限、预算、占用限制的调度，忙时表达等待；智能设置提供显式建立/更新索引及取消入口，复用当前能力配置/索引会话，无自动发送私人文本。
- [ ] 空查询给推荐分配有限名额；在RAM保留面板尺寸已经由Task2完成，不重复修改。检查关闭各功能后的入口和状态一致性。
- [ ] bump至0.1.1并更新中文README/验证说明。运行一次完整host共享套件`cargo test --locked -- --test-threads=1`、相关严格门禁与core-check；未改Node模块不重跑Node suite于host（Windows CI本身会运行其正常流水线）。以明确新失败驱动定点修复，不能重复全套刷结果。
- [ ] commit后由root运行验证分支的完整Windows CI；真实全流程通过且最终review通过后更新main。修复任何阻挡该流程的具体问题，不跳过原生测试，真实环境限制需如实说明而非假绿。
- [ ] 为最终已提交源码构建Windows Release或采用同commit CI产生的可核验包，保留已核验Node/Pi资产与许可。fresh ZIP精确成员/CRC/SHA/bytes/source-stage一致，manifest记录source_commit/clean，SHA256SUMS一致；由root发布新的v0.1.1预览版，不覆盖v0.1.0。报告提供完整命令metadata/raw logs、host/Windows实际计数、源码SHA/包SHA和仍需人工Windows验收范围。
