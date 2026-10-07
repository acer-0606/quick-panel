# 交互与设置清晰化 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复完整交互审查的全部22项，将入口、设置、停用、执行与错误恢复统一为可理解的行为。

**Architecture:** 继续Win32原生页面与actor存储，将可测试的表单/状态决策放在小型共享Rust模块，原生消费者实际复用。四个实现任务串行处理表单基础、管家记忆、主操作状态、设置中心与交付，每个任务单独审查。

**Tech Stack:** Rust2024、Win32 windows0.62.2、SQLite、现有Pi Core1.0.4/Node24.19.0，不新增运行时或依赖。

**Spec:** `docs/superpowers/specs/2026-10-07-ux-clarity-design.md`

## Global Constraints

- 交互与文档使用中文；Windows 10/11 x64 为主要产品平台，继续 Rust/Win32 原生控件、现有 theme、SQLite 和固定 Pi 运行时，不新增 WebView、Electron、UI框架或全局热键。
- 保持历史/后台派生数据的低频批量写入和正常退出保存；收藏、配置、人工确认立即保存。只读状态、绘制、设置导航不提交事务、不读正文/BLOB、不请求网络。
- 不访问真实 API、密钥、用户数据库、正文、剪贴板或屏幕；验证使用合成数据。macOS 的共享/进程测试与 Windows 交叉编译不声称 Windows 实机运行。
- 保留已有实际任务占用、代次/来源/版本/回执守卫、IME原生编辑、草稿保留、滚动位置、固定footer和DPI行为；关闭窗口/退出不等待网络或join子进程。
- 关闭AI检索不连带关闭独立AI管家；关闭记忆停止使用已有记忆和新反馈，明确告知而不悄悄重开。关闭管家停止后台整理，允许管理已有建议与记忆。
- 用户明确发起的API失败只产生本次安静的状态说明，不重复弹错、不显示服务原始正文；提供对应设置/连接测试路径。
- 同时最多一个实现者和一个Cargo命令；固定 `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`。定点回归后才扩大检查，完整共享全套在最终集成源码上执行一次，后续仅新变更/失败才重跑必要范围。
- 仓库 `/Users/chuwujie/workspace/quick-panel`、现分支 `codex/quick-panel`。不创建worktree、不push、不操作用户环境或全局配置。root维护docs/scratch，不直接修实现；你不是唯一会接触代码的人，不回退他人编辑。
- 本机Node固定 `/Users/chuwujie/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin/node`；Rust测试设置 `QP_TEST_NODE` 指向它；全局旧Node不可用，不修复它。Pi bundle已经存在，不修改Node协议/代码。

---

### Task 1: 表单校验、输入恢复与设置提示基础

**Files:** 修改 `src/platform/windows/dialogs.rs`、`src/platform/windows/ui.rs`（仅表单effects/apply与恢复路径）、`src/organizer_ui.rs`（已有解析/effort helpers）；按需新增 `src/form_validation.rs` 并在 `src/lib.rs` 导出；测试 `tests/form_validation.rs`、`tests/organizer_ui.rs`、`tests/hotkeys.rs`、`tests/connection_test.rs`。

**Interfaces:** 消费现有 `settings_form/ai_settings_form/embedding_settings_form/path_settings_form/screenshot_settings_form` 的原生字段与 `Option<Vec<String>>` 返回、`ClipboardMetadataForm::finish`、`parse_ai_fields`、`effort_warning`、`parse_hotkey`。保留这些当前字段顺序/返回合同，Task4会在复审后重组分组。若引入共享校验，生产native必须使用该函数，报告精确类型/签名供Task4沿用。

**Requirements:** 完成UX02/03/04/18。所有基础表单关闭前做可执行本地校验，错误保持raw输入/选择/IME，明确错误字段并滚入；不要把原值重新填入来覆盖草稿。Embedding验证地址/模型/启用时已有或新key、互斥清除及格式，保存不调用API。关闭AI跳过仅运行需要的effort支持校验；开启或手动测试继续校验。后续hotkey注册/DPAPI错误保留本次草稿可继续修正，敏感key不进日志/存储草稿、不带入Node。管家依赖提示/保存错误/测试结果分层，轮询只更新测试区域，不覆盖错误。路径选项文案明确“清空访问统计及不再推荐名单”，保留存储语义。

- [ ] 先读上述函数和调用者，写失败用例覆盖：(a)已知unsupported effort但关闭AI可保存；(b)开启仍拒绝；(c)基础/Embedding错误保留raw和错误字段；(d)runtime health不得覆盖配置/保存错误；(e)native提交失败可恢复相同草稿。
- [ ] 为复杂状态新增实际生产使用的便携策略，最小合同示例（最终可采用既有等价类型）：

```rust
pub struct FieldError { pub field: usize, pub message: String }
pub fn unsupported_effort_error(enabled: bool, warning: Option<String>) -> Option<String> {
    enabled.then_some(warning).flatten()
}
#[test]
fn disabling_ai_does_not_require_a_supported_effort() {
    assert_eq!(unsupported_effort_error(false, Some("unsupported".into())), None);
    assert!(unsupported_effort_error(true, Some("unsupported".into())).is_some());
}
```

- [ ] 先运行定点用例得到行为RED，实施最小校验/状态/草稿生命周期变更，再运行GREEN；不为文案单独写镜像测试。测试应实际调用native消费的共享校验，不另造平行实现。
- [ ] 命令：`cargo test --test form_validation --test organizer_ui --test hotkeys --test connection_test`（若未新增文件去掉对应target）；全部exit0后 `cargo fmt --all -- --check`、host严格Clippy和Windows all-target严格Clippy/check。Cargo严格串行。此阶段不跑完整suite/Release。
- [ ] 自审窗口取消、关闭、测试busy、OS失败与回执恢复；只提交owned文件，写task-1-report.md包含变更/精确接口/RED→GREEN命令输出/平台边界，返回DONE或具体疑问。

### Task 2: 管家与记忆的状态、操作及影响说明

**Files:** 修改 `src/organizer_display.rs`、`src/organizer_store.rs`、`src/organizer_ui.rs`、`src/organizer.rs`、`src/harness_tools.rs`、`src/harness_store.rs`、`src/engine.rs`（只读状态/人工校验相关）、`src/platform/windows/organizer_ui.rs`、`harness_memory_ui.rs`、`ui.rs`（native管理跳转/状态接线）、`dialogs.rs`（开关/模式文案）；对应 `tests/organizer_display.rs`、`organizer_ui.rs`、`organizer_engine.rs`、`harness_engine.rs`、`palette.rs`（若必要）。

**Interfaces:** 消费Task1现有表单/状态修复，报告由root提供；消费`MemorySnapshot`、`MemoryRecord`、`ReviewModel`、`Progress`、`Stage`、`HarnessMemoryReceipt`。可扩展只读snapshot为当前organizer/memory启用状态以及Progress的Attention投影；不得改变MemoryRecord持久格式、工具授权、回执幂等或来源版本约束。新增返回/字段必须在报告给出精确签名及后续Task3/4接线要求。

**Requirements:** 完成UX01/05/06/08/09/19/20。反馈模式去掉不生效的启用/确认控件并明确保存即启用/所见拒绝名/范围；真实记忆管理的checkbox必须保留并读值。所有类型、alias/tags在详情可完整阅读，Rejected扩大范围显示实际影响。记忆总开关或管家关闭要显著说明，已有管理可用，反馈未启用时先引导设置而非允许填完再拒绝。Attention主入口计数及逐条阶段可见，详情展示原因/来源/恢复方式，不能按Ready应用。“忽略建议”准确说明当前版本与不学习语义。人工别名/标签校验返回字段错误与上限，支持原始草稿；新增标签/模式文案准确表达追加。

- [ ] 在真实生产状态模型写RED：Attention-only Progress不应暂无待办；Attention详情包含完整原因；记忆关闭时实际生效为false且明确banner；反馈模式字段不会虚构可切换确认；拒绝记忆详情含alias；人工错误指向字段。
- [ ] 延伸既有真实SQLite用例验证只读projection不脏写、关闭记忆不使用负向规则、管理记录不丢失；不要重写已正确authority合同或新增AI权限。
- [ ] 生产实现遵循已有字段和读取方式，示例状态选择必须被native消费：

```rust
// 将Attention纳入Stage/Progress；footer优先显示待处理而不伪报空。
let attention = progress.items.iter().filter(|(_, stage)| *stage == Stage::Attention).count();
if attention > 0 { text.push_str(&format!(" · 需处理 {attention} 条")); }
// 人工校验采用明确字段，不复用针对模型输出的泛化文案。
if alias.chars().count() > 48 { return Err("别名最多48字符，请缩短后再应用".into()); }
```

- [ ] 定点运行 `cargo test --test organizer_display --test organizer_ui --test harness_engine --test organizer_engine`，先限定新增用例再必要覆盖本组已有回归；含actor用例固定串行。随后host/Windows严格检查，fmt/diff，无完整suite/Release。
- [ ] 自审scope/metadata不可修改错对象、显示原文仅本地已有投影、失败保存保留草稿、关闭后管理及设置返回；提交owned文件，完整task-2-report.md，接口供Task3/4消费。

### Task 3: 主操作、智能检索与索引的持续状态

**Files:** 修改 `src/platform/windows/ui.rs`、`src/query_feedback.rs`、`src/smart_search.rs`、`src/semantic_ui.rs`、`src/ui_state.rs`、`src/palette.rs`、`src/screenshot.rs`（若需通知决策）；按需新增 `src/activity_status.rs` 与lib导出；测试 `tests/query_feedback.rs`、`smart_search.rs`、`semantic_ui.rs`、`ui_state.rs`、`preview.rs`、`screenshot.rs`、`palette.rs`。

**Interfaces:** 消费Task2 reviewed `Progress/Stage`扩展及Task1表单恢复；保持query token/profile、worker实际占用、StatusCache、SmartPreflight、index accepted receipt。新增会话显示状态是RAM投影，不能写SQLite或添加持续线程。Task4可读取这些状态形成设置中心的说明，报告精确helper签名。

**Requirements:** 完成UX10/11/12/13/14/15/16/17/21。暂停记录有持久标识和状态化托盘菜单；不能被selection/Saved覆盖。收起预览调用完整清理并让所有异步入口拒绝隐藏预览结果，内存释放而实际worker占用规则不变。Claude显式绕过索引预检；Auto/Semantic保持有效索引/无索引引导。确定实际能力后2048/4096校验，保留实际能力标识直到新查询或取消。失败一次安静状态、不含原服务错误，提供设置可达路径；本地结果保持可操作。index停止三态分开，取消刷新当前profile状态并显示保留数；空范围/0完成解释可索引内容条件。截图以不抢焦点的原生可见反馈报告处理中/完成/未入历史，并不覆盖新剪贴板或误宣称复制成功。

- [ ] 为生产状态写行为RED：Claude+Embedding缺status仍能选择Claude；取消index后刷新并保留accepted count；0完成不要求无限重建；raw API错误不泄漏且可见neutral status；选择项不抹掉会话/暂停；关闭preview迟到结果无副作用。
- [ ] 样例期望（使用实际当前SmartView构造器/字段）：

```rust
let mut preflight = SmartPreflight::default();
let view = SmartView { preferred: SmartSearchMode::Claude, ai_enabled: true,
    embedding_enabled: true, query: "找昨天的记录".into(), scope: Scope::All,
    profile: "fixture".into() };
preflight.begin(view.clone());
assert_eq!(preflight.finish(&view, true, None), Some(SmartAction::Claude));
```

- [ ] 实施并定点GREEN；新增状态生命周期调用必须来自实际native路径，不能只测helper而漏接线。截图通知仅用户主动操作，不新增后台提示风暴。
- [ ] `cargo test --test query_feedback --test smart_search --test semantic_ui --test ui_state --test preview --test screenshot --test palette`；随后host/Windows严格检查、fmt/diff。此阶段不重复完整suite/Release、Node suite或基准。
- [ ] 自审重复点击、已完成智能结果、新查询/隐藏/修改配置、index后台运行、取消后迟到返回、退出状态优先级；提交并写task-3-report.md。

### Task 4: 统一设置中心、数据保留、分组与最终交付

**Files:** 新增 `src/settings_ui.rs`（共享分组/可见状态/保留解析/布局策略）、`src/platform/windows/settings_ui.rs`（原生设置中心）；修改 `src/lib.rs`、`src/platform/windows/mod.rs`、`ui.rs`、`dialogs.rs`、`src/palette.rs`、必要`src/model.rs`（只保留已有存储字段）；测试 `tests/settings_ui.rs`、`palette.rs`、`hotkeys.rs`、`persistence.rs`、`form_validation.rs`及受接口影响fixtures；更新README/docs验证。打包只使用现有scripts，不改协议或供应链。

**Interfaces:** 消费前三任务复审后的表单校验/状态/设置跳转API，root提供最新context报告。保留Settings已有retention_days/max_history/三个热键/AI/Embedding/organizer字段和serde兼容。设置入口改变为统一原生hub，叶子表单可复用；必须让当前返回路径保持分组上下文，App消息/回执继续泵送，无阻塞join/重复注册。

**Requirements:** 完成UX07/22并集成所有22项。主按钮、托盘、搜索“设置”目标一致；6组严格按spec，设置列表不混区域截图等立即操作。全部三组快捷键同组，数据页包含暂停/排除/保留，智能查找同组包含功能启用/首选/索引范围与状态，服务连接清楚是Claude共用连接和独立Embedding。已有管理入口显示停用，配置入口保留；双智能关闭按钮表示启用/配置。高级管家调度参数集中高级入口，核心模式/范围与开关易见。不能出现两个页面有互相覆盖的重复开关或旧叶子提交覆盖别组新配置。

保留策略：数据页显示未固定历史“30天未使用/最多10000条”的当前实际值（不写死defaults显示）；允许正整数调整，天数1–36500、条数1–1000000，溢出/0/非数字明确拒绝并保留草稿。保存前持续说明减少限额会按策略清理未固定历史，固定/收藏保留。验证真实SQLite清理语义不改默认，不在导航/打开页时清理或写盘；现存非常规值可显示且不因只修改其他设置被静默归一化。

原生中心视觉完全遵循spec的theme/Segoe UI/左分组右详情与固定footer。采用可滚动内容/焦点滚入，不让长中文、关闭状态说明或高DPI挤掉动作。复用叶子编辑时有明确返回路径及上下文，不创建web模拟UI充数。

- [ ] 先写生产分组/路由与状态策略RED：三个设置入口同hub；6类互不混操作；关功能显示准确说明并保持配置可达；热键跨三组冲突保留旧值；保留输入边界/SQLite既有固定免清理。布局测试只对真实native消费的geometry做DPI96/144/192、小屏360×480及800×600可达检查。
- [ ] 保留数值校验最小实现逻辑（复用T1 FieldError）：

```rust
fn positive(value: &str, max: usize) -> Result<usize, String> {
    value.trim().parse::<usize>().ok().filter(|n| (1..=max).contains(n))
        .ok_or_else(|| format!("请输入1至{max}的整数"))
}
#[test]
fn invalid_retention_does_not_become_zero() {
    for value in ["", "0", "-1", "abc", "9999999999999999999999999"] {
        assert!(positive(value, 36500).is_err());
    }
    assert_eq!(positive("30", 36500).unwrap(), 30);
}
```

- [ ] 定点GREEN后复核全22项source/消费方映射及模块启用/禁用矩阵，更新README实际使用导航。完整共享测试前冻结生产源码。
- [ ] 串行执行最终 `cargo test --locked -- --test-threads=1`（QP_TEST_NODE固定），读取完整输出确认exit0；host/Windows all-target strictClippy及check、fmt/diff；`cargo build --release --locked --target x86_64-pc-windows-gnu`、`cargo run --release --locked -- --core-check`。每条明确完成后才启动下一条。Node源码不变不重跑30项Node suite。
- [ ] 从本轮scratch/node-assets使用已验证node.exe+LICENSE，`python3 scripts/package-portable.py --node-assets .superpowers/sdd/2026-10-07-ux-clarity/node-assets`，fresh ZIP/manifest/canonicalchecksums需与最后源码文档一致，不含scratch。记录实际ZIP/EXE/BUNDLE/Node bytes+SHA/CRC/member对齐及Windows实际0边界。
- [ ] 自审导航返回、modal/输入法、DPI、小屏、生命周期、状态错误优先级及22项完成表；提交所有owned实现+文档，完整task-4-report.md记录每次定点/完整命令、exit、输出、产物。root随后派任务审查及一次最终全功能审查。
