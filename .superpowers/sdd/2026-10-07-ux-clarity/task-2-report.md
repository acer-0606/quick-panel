# Task 2：管家与记忆的状态、操作及影响说明

状态：DONE。基线 `ffd97476e2285763e8401e26458dde9960d23997`，分支 `codex/quick-panel`，原目录实施；提交主题 `fix: clarify organizer attention and memory operations`。

## 七项验收

| 项目 | 已落地行为 | 证据与边界 |
|---|---|---|
| UX01 | 反馈模式不创建启用/确认 checkbox；零尺寸 STATIC 占位仅保持既有控件索引和几何。只留原因、范围与“保存反馈并重做”；详情明说保存即启用、完整所见被拒名称、当前选择范围和扩大影响。真实管理模式仍创建 checkbox，保存读 BM_GETCHECK。 | `clarity_feedback_intro_explains_immediate_activation_and_has_no_confirm_fields`；native 两分支创建与保存源码自审，Windows 编译。 |
| UX05 | actor Attention 投影为 `Stage::Attention`，Progress 计数、现有 palette 主行 badge 自动消费。仅 Attention 不再“暂无待办”；建议状态也显示需处理。选择 Attention 展示完整标题、来源 id/路径、所见版本、code/message 与重试/反馈恢复方法。仍没有可应用 Proposal，也不能 check/apply。 | actor 原用例新增实际 Progress 断言；`clarity_attention_badge_and_count_survive_ready_and_paused_states`；既有 Attention UI 用例延伸完整详情断言。 |
| UX06 | 列表显示规则/采用示例/修改示例/拒绝示例/自动结果类型；滚动详情完整显示别名、标签、文本、来源及原 AI provenance、范围、依据、单条配置和当前是否生效。选范围和编辑规则时预览完整扩大约束，Rejected 明示被拒别名；保存读取真实选择，保留原范围/全局的既有合法路径，不为另一对象生成范围。 | `clarity_memory_detail_discloses_rejected_alias_tags_scope_and_disabled_effect` 覆盖完整拒绝别名、标签、文本和全局影响；native scope_preview 消费实际编辑文本。 |
| UX08 | “暂不处理”改成“忽略建议”；已选建议完整详情说明仅忽略当前版本、不学习偏好、普通重试不会重新出现。 | 仅文案变更，沿用 Done/所见版本和既有 dismiss actor 回归，不新增队列。 |
| UX09 | 开关为“启用整理记忆（使用规则并记录反馈）”。只读 snapshot 提供两个父开关；管理页持续区分单条已配置与当前不生效，关闭后仍可管理。反馈页在填表前显示停用原因，禁用填写、选范围和保存，首次焦点到“管家设置”；未保存 pending 的真实保存重试仍可用。设置保存或取消后返回原顶层管理页。 | 启用矩阵 shared helper 测试，真实 SQLite 关闭记忆停止负向约束/保留记录/只读不提交；既有关闭后管理与同 receipt 保存重试覆盖保持通过。 |
| UX19 | `Proposal::validate_manual` 返回可修正的别名 1–48、标签最多 4 个/每个 1–24 字符、重复/非法控制字符等字段错误。ReviewState 在提交前校验；actor 人工 apply 在生成采用记忆之前复验。模型/持久 validate 不变。原始空白和尾逗号草稿不被覆盖。 | 两项真实 ReviewState RED→GREEN；原草稿/所见版本/失败回执回归保持通过。 |
| UX20 | 人工标签字段为“新增标签”，详情明确追加且留空不删除原标签；自动模式为“后台自动：仅填空别名，追加标签”。 | 实际合并逻辑不改，既有自动与人工 metadata/tag 合并回归通过。 |

## 只读 DTO 与后续 Task 3/4 接口

`src/harness_tools.rs`：新增两个只读字段；`MemoryRecord` 的持久格式、Tool DTO 和 authority 没有改变。

```rust
pub struct MemorySnapshot {
    pub organizer_enabled: bool, // 新增；actor 从 Settings.organizer.enabled 填入
    pub memory_enabled: bool,    // 新增；actor 从 Settings.organizer.memory_enabled 填入
    pub source_labels: Vec<(Source, String)>,
    pub version: u64,
    pub records: Vec<MemoryRecord>,
    pub storage_error: Option<String>,
}
impl MemorySnapshot {
    pub fn feedback_available(&self) -> bool;
    pub fn record_effective(&self, record: &MemoryRecord) -> bool;
    pub fn availability(&self) -> String;
}
impl MemoryKind { pub fn label(self) -> &'static str; }
```

`feedback_available` 为两个父开关均开启且缓存未损坏。`record_effective` 还要求本条 enabled、confirmed、origin=Human；不写回这些标志。这里是当前配置下所选范围的可生效投影，不扫描来源内容、不请求模型。HarnessStore 内部无 Settings，snapshot 初始化两个字段为 false；唯一 actor 对外出口填入真实值。native 打开窗口前也从本地当前 Settings 填值并增加 revision，避免首帧依赖异步结果。

`src/organizer_display.rs`：`Stage` 新增 `Attention`，`Stage::label()` 返回“需处理”；`Progress` 结构签名不变，items 现在包含 Attention。Task 3 的主状态和 Task 4 的设置中心应消费 `Progress::label()` 或同一 Stage，不能把它当 Ready、漏计，或将 Attention-only 投影为空待办。

`src/organizer.rs`：新增 `pub fn Proposal::validate_manual(&self) -> Result<(), String>`；只供人工字段校验，原 `validate()` 保持模型/持久边界。

`src/organizer_ui.rs` 新出口（全部仅使用内存投影）：

```rust
impl ReviewState { pub fn attention_detail(&self) -> Option<String>; }
pub fn memory_scope_label(scope: &MemoryScope, snapshot: &MemorySnapshot) -> String;
pub fn source_label(source: &Source) -> String;
pub fn memory_detail(record: &MemoryRecord, snapshot: &MemorySnapshot) -> String;
pub fn memory_scope_impact(record: &MemoryRecord, scope: &MemoryScope,
                           snapshot: &MemorySnapshot) -> String;
pub fn feedback_intro(title: &str, target: &FeedbackTarget) -> String;

#[derive(Clone, Debug)]
pub struct MemoryNavigationDraft {
    pub target: Option<FeedbackTarget>,
    pub record: Option<MemoryRecord>,
    pub raw_input: String,
    pub scope_choice: usize,
    pub enabled: bool,
    pub confirmed: bool,
}
// 新增公开内存字段，不序列化、不作为真实已提交 command：
pub struct MemoryUiState {
    pub navigation_draft: Option<MemoryNavigationDraft>,
    // 其他已有字段保持
}
impl MemoryUiState {
    pub fn navigation_draft_for(
        &self, target: Option<&FeedbackTarget>
    ) -> Option<MemoryNavigationDraft>;
}
```

`navigation_draft_for` 只在无 pending 且精确所见 source/version/rejected_alias 匹配时恢复；pending 有自己的既有 raw/command 恢复路径，成功 receipt 清除导航草稿。设置返回恢复 raw、原 scope choice 和管理 checkbox，不把旧目标草稿绑定到新别名。新增 `clarity_settings_navigation_keeps_raw_feedback_bound_to_exact_seen_target` 覆盖这些条件和 snapshot adoption 不丢草稿。

Windows 公开函数仅新增 bool 返回（参数顺序未变）：

```rust
// src/platform/windows/harness_memory_ui.rs
pub unsafe fn show(
    owner: HWND, state: Rc<RefCell<MemoryUiState>>,
    submit: Rc<dyn Fn(Command)>, appearance: Theme
) -> bool;
pub unsafe fn feedback(
    owner: HWND, state: Rc<RefCell<MemoryUiState>>,
    submit: Rc<dyn Fn(Command)>, appearance: Theme,
    target: (String, FeedbackTarget)
) -> bool;
// src/platform/windows/organizer_ui.rs
pub unsafe fn show(
    owner: HWND, state: Rc<RefCell<ReviewState>>,
    memory: Rc<RefCell<MemoryUiState>>, submit: Rc<dyn Fn(Command)>,
    appearance: Theme, enabled: bool
) -> bool;
```

返回 true 为用户请求管家设置，窗口已关闭；false 为普通关闭/创建失败。Host 效果泵在释放窗口 borrow 后进入设置，不在子窗口持有 App 借用时打开 native dialog。内部 `OrganizerManagement::{Memory, Review}` 与 `UiEffect::OrganizerSettings.return_to: Option<OrganizerManagement>` 保留返回目的地。直接记忆页返回记忆页；建议窗口内反馈/记忆子页从设置返回建议父页，再点对应入口恢复匹配草稿。Task 4 重组设置中心时应保留此明确返回目的地和 exact-target 草稿合同。`organizer_settings_form` 公共签名、Task 1 原窗口 Submit/错误 footer/密码擦除合同均未改。

## TDD 实际命令与输出

所有测试/Clippy/check 均使用固定环境前缀：

```sh
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 QP_TEST_NODE=/Users/chuwujie/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin/node
```

Cargo 严格串行；明确取得终态 exit 后才开始下一条。预期 RED 的 exit 101 记录后进入修复/下一独立 RED，不当成成功。

1. 人工字段 RED，生产实现前：

```sh
cargo test --test organizer_ui clarity_manual -- --test-threads=1
```

编译成功；2 failed，均输出 `AI 返回的整理建议无效`，`0 passed; 2 failed; 32 filtered out`，exit **101**。预期是现有人工校验复用了模型错误；别名/标签字段文本断言失败，raw 草稿断言由 GREEN 进一步确认。实现后同命令：`2 passed; 0 failed; 32 filtered out`，exit **0**。

2. Attention actor RED，生产投影修复前：

```sh
cargo test --test harness_engine feedback_on_processing_and_attention_without_displayed_alias_never_learns_hidden_staged_alias -- --exact --test-threads=1
```

编译成功，实际 actor Attention-only Progress 输出 `AI 管家 · 暂无待办`，`0 passed; 1 failed; 31 filtered out`，exit **101**。加入 Stage/只读投影与计数后同命令：`1 passed; 0 failed`，exit **0**。

3. 先将 native 已有 scope/详情/反馈说明抽为实际生产共用 helpers，保留原行为；为只读 DTO 添加默认 false 字段，尚未接 actor。补拒绝别名、实际启用和即时保存反馈断言，再跑行为 RED：

```sh
cargo test --test organizer_ui clarity_ -- --test-threads=1
```

`2 passed; 2 failed`，exit **101**。关闭记忆仍 `record_effective=true` 的断言失败；反馈 intro 仍为原“所见建议…需先启用记住整理反馈”，不含保存即启用。不是未定义接口/编译错误的 RED。

4. Attention 原 native fallback 详情 RED：

```sh
cargo test --test organizer_ui attention_rows_remain_non_applicable_and_status_explains_local_recovery -- --exact --test-threads=1
```

编译成功，实际 helper 输出 `选择一条建议查看原始名称、完整路径或文本摘要`，完整原因/来源断言失败，`0 passed; 1 failed`，exit **101**。

5. actor 对外启用状态 RED：

```sh
cargo test --test harness_engine feedback_persists_reason_scope_negative_alias_and_duplicate_request_is_idempotent -- --exact --test-threads=1
```

编译成功，真实启用设置的 snapshot 仍两个字段 false，`assertion failed: m.organizer_enabled && m.memory_enabled`，`0 passed; 1 failed`，exit **101**。

GREEN：字段接 actor、完整详情和模式说明实现后，`cargo test --test organizer_ui -- --test-threads=1`：**36 passed，exit 0**。中途一次 clarity 文案断言仍失败，因为文案写“被拒”而测试要求完整“拒绝原因”；已改成用户字段“拒绝原因”，随后全组通过。

延伸第 5 条真实 SQLite 用例，加关闭记忆后的已保存记录、只读 data_version、负向别名停止使用和管家关闭状态断言后，同 exact 命令：**1 passed，exit 0**。

必要本组回归：

```sh
cargo test --test organizer_display --test organizer_ui --test harness_engine --test organizer_engine -- --test-threads=1
```

实际输出：`harness_engine: 32 passed`，`organizer_display: 7 passed`，`organizer_engine: 29 passed`（83.79s），`organizer_ui: 36 passed`；**104 passed，exit 0，无警告**。新增导航草稿测试/helper 后仅重跑受影响 UI 目标：

```sh
cargo test --test organizer_ui -- --test-threads=1
```

最终 **37 passed，exit 0**；本组唯一用例累计 **105**。新增测试最初误用外部 struct update 构造含 private 字段的 MemoryUiState，编译报 E0451；改成 Default 后赋公开 navigation_draft，最终测试通过。该编译失败不是行为 RED。最终 actor/display 代码与 104 项时相同，新增共享代码只为导航 draft 读取，未无理由重跑全部目标。

## 最终严格检查

- `cargo fmt --all`：exit 0，规范化 owned 源码。
- `cargo clippy --all-targets -- -D warnings`：最终 exit 0，`Finished dev profile ... 7.93s`，无警告。
- `cargo clippy --target x86_64-pc-windows-gnu --all-targets -- -D warnings`：最终 exit 0，`Finished dev profile ... 10.58s`，无警告。首次失败仅两个窗口 bool 返回的 `let_and_return` lint，修为无 callback 的最终直接 bool 表达式。修后验证为最终源码。
- `cargo check --target x86_64-pc-windows-gnu --all-targets`：exit 0，`Finished dev profile ... 8.01s`，无警告。
- `cargo fmt --all -- --check`、`git diff --check`：见最终落盘检查记录（本报告后追加结果）。

没有运行 full suite、Release 或 Pi Node suite；没有访问真实 API、用户数据库、密钥、正文、剪贴板或屏幕。Node 未修改，SQLite 全部为 tempfile 合成数据。

## 自审、文件与边界

- 存储只增加只读 snapshot 默认字段，没有 MemoryRecord serde 变化、工具授权变化或 dirty/写入路径变化。actor 仅读取当前 Settings；现有 source_labels 继续 metadata-only 有界读取。
- `memory_scope_impact` 在范围变化与编辑文本变化时被原生完整可滚动详情消费；Rejected 别名/标签不会在扩大到全局时被隐藏。没有新建范围授权路径，非法 item↔directory 仍返回原错误且保留 raw。
- 管家关闭不清除已有建议/记忆；记忆关闭不改 per-record enabled/confirmed。已有 pending 保存重试维持原 ID/签名，父开关关闭不阻止已接受操作落盘。
- callback、SetWindowText、SendMessage、theme::apply 等会引起同步消息的调用前，拿出 owned handles/String/state/commands 后释放相关 RefCell。修了反馈 pending let-chain 跨原生回填的旧 borrow，以及 native submit match/重试持有临时 RefMut 的模式；没有恢复 Task 1 effort-warning 借用问题。对初始 EDIT 的 EN_CHANGE，scope_preview 先验证 controls.len，避免控件尚未创建完就索引。
- 反馈 checkbox 不再存在为可用控件；管理 checkbox 原读值保持。复用既有 7 fields/4 footer buttons 与 memory_layout；状态/详情使用带滚动的只读原生 EDIT，长停用说明不会只能被截断。DPI、原生 IME 策略、实际后台占用、滚动与固定 footer 未改；所有跨线程/回执守卫继续由原方法处理。
- 追加标签/忽略说明仅改用户语义展示，原合并和 Done 行为没有扩展。
- 文件较大的 `organizer_ui.rs`、Windows `ui.rs`/dialogs 仍沿用现有组织，仅在授权范围做 focused 修改。Task 4 可按既定计划消费这些接口，不在本任务重构文件。
- Windows 实机视觉、IME、焦点、DPI/多屏实际运行均未执行；证据是可移植生产模型回归、native 源码自审和 Windows 严格交叉检查，不声称 Windows 实机验收。

Owned 文件：`src/organizer_display.rs`、`src/organizer_store.rs`、`src/organizer_ui.rs`、`src/organizer.rs`、`src/harness_tools.rs`、`src/harness_store.rs`、`src/engine.rs`、`src/platform/windows/organizer_ui.rs`、`src/platform/windows/harness_memory_ui.rs`、`src/platform/windows/ui.rs`、`src/platform/windows/dialogs.rs`、`tests/organizer_display.rs`、`tests/organizer_ui.rs`、`tests/harness_engine.rs` 和本报告。`tests/organizer_engine.rs` 只运行，未改。root docs/scratch 未编辑。未创建子代理/reviewer；独立复审由 root 发起。

最终落盘检查：`cargo fmt --all -- --check` exit **0**，无输出；`git diff --check` exit **0**，无输出。报告与 owned 代码一起提交。

## Fix round 1 / 5：I1 失败 pending 下的设置入口与返回草稿

FIX_BASE：`bdc574acc546b877f92b519bf3ce0b490e8f475e`。修复主题：`fix: keep disabled feedback settings reachable after failure`。读取完整 `task-2-review.md` 后核对 I1：`receipt(Err)` 保留 pending，native 唯一设置槽被 pending 改成 retry；父开关关闭时输入/Save 禁用，页内只有重发原失败 command，确实缺少 banner 承诺的设置路径。

本轮修复：

- 抽出 native 实际消费的四槽 footer 模型及 route，先保持原行为运行 RED；随后让反馈模式的 slot 1 保持 retry，slot 2（原隐藏删除位置）独立显示“管家设置”。render 的标签/显隐/可用性、点击 route、停用首焦点均消费同一策略，不在测试另写一套 native 分支。
- 未接受失败的反馈 retry 明确为“重试上次提交”；已接受待落盘为“重试保存”。设置路由不清空 pending、不改变原 command，不把新编辑作为 retry 的原参数；正在执行 retry 时入口禁用，完成失败回执后再次可达。原真实管理模式的删除与设置/retry 行为保持。
- 复用原 `MemoryLayout.buttons: [MemoryRect; 4]` 与控件 7–10；移除反馈 resize 时强制隐藏 slot 2 的旧代码。没有增加按钮或改变 footer/viewport 几何。
- 新设置路径直接暴露的返回边界也修复：只有未接受失败、非 saving、非 pending_save、从未 semantic_admitted 且 pending 是**精确同一 FeedbackTarget** 的反馈时，才能恢复导航里的新未提交 raw/范围。原 command/raw 仍独立保存，可重试上次提交。已接受操作即使后来得到 Err(false) 也不解除绑定。
- native 显式先取得合格 navigation draft，仅没有合格 draft 时回填原 pending；随后一次应用选中的 draft，避免旧 pending 覆盖新草稿。开启父开关后，正常 Save 可以用新 request ID 提交当前新草稿。管理对象与不同所见版本/alias/source 不获得新增恢复通路。

### 本轮实际 RED/GREEN

仍用报告前述固定环境前缀、固定 `QP_TEST_NODE`，Cargo 严格串行。预期行为 RED 先记录 exit 101，修复或下一独立 RED 再继续；普通检查以明确 exit 0 为下一命令前提。

I1 RED：从生产 `MemoryUiState::submit → receipt(Err(false/true)) → request_snapshot/adopt(父开关关闭)` 构造真实状态，调用已经由 native 消费、保留原行为的 footer 策略：

```sh
cargo test --test organizer_ui failed_feedback_with_disabled_parent_keeps_settings_and_original_retry_reachable -- --exact --test-threads=1
```

编译成功，实际输出 `停用反馈的失败 pending 仍需独立设置入口`；`0 passed; 1 failed; 37 filtered out`，exit **101**。修复后同命令 `1 passed; 0 failed; 37 filtered out`，exit **0**。覆盖 Err(false/true) × 管家关/记忆关/同时关闭六种状态；设置与 retry 独立可见可达、Save 被禁用；原 request ID/source/version/rejected_alias/reason/scope 和 raw/pending_save 不变。

直接相关设置返回 RED（已向 root 说明并获准纳入）：

```sh
cargo test --test organizer_ui settings_return_restores_only_unaccepted_feedback_edits_without_changing_retry -- --exact --test-threads=1
```

原 helper 一律排除 pending，实际输出 `未接受失败的新编辑应跨设置往返保留`；`0 passed; 1 failed; 39 filtered out`，exit **101**。加入限定恢复后同 exact 命令 `1 passed; 0 failed; 39 filtered out`，exit **0**。

root 补充要求明确 retry 的旧提交语义后，同 exact 用例添加真实 footer label 与新草稿 Save 断言。再次实际 RED：left `重试保存`、right `重试上次提交`，`0 passed; 1 failed; 39 filtered out`，exit **101**。修正生产 label 并显式调整 native 恢复优先级后，最终运行：

```sh
cargo test --test organizer_ui -- --test-threads=1
```

最终输出 `Finished test profile ... 5.63s`，**40 passed; 0 failed**，exit **0**，无警告。此前本轮 39 项及中间 40 项也通过；这里只以最后源码 40 项为最终证据，没有重跑或声称重跑其他 actor/full 目标。

测试矩阵：

| 状态/触发 | 覆盖结果 |
|---|---|
| Err(false) + 每一种父开关停用 | 独立设置、重试上次提交；原 command/raw 保留，新 Save 暂不可用 |
| Err(true) + 每一种父开关停用 | 独立设置、原 ID/签名参数的保存 retry，pending_save 保留 |
| 未接受失败后编辑 raw/范围，再经设置返回 | 精确目标恢复新未提交草稿；retry 仍为原提交；恢复父开关后正常 Save 可提交新 raw/Global 范围和新 ID |
| 已接受 Err(true) 后又收到 Err(false) | semantic_admitted 不被撤销；忽略伪造的新导航草稿，继续绑定原 command/raw |
| 导航草稿改为另一所见版本 | helper 拒绝恢复，原 retry 目标不变 |
| 正在实际 retry | 设置/retry 不重复进入；关闭仍可用 |
| 真实管理模式 | 原 Save、设置、删除仍可达；pending_save 时保留 retry、禁用删除 |
| 新反馈尚无 pending | retry 不虚构可执行命令，独立设置仍可达 |
| DPI 96/144/192，320×280、640×360、1000×620 | 用实际 `memory_layout` 验证 settings/retry 所在 slot 有正尺寸、留在 footer、屏幕边界内；已有小屏/反向焦点/滚动/IME 回归继续通过 |

### 新增接口与合同差异

`src/organizer_ui.rs` 新增（公开、仅内存读取，不请求 actor、不写存储）：

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryFooterAction { Save, Retry, Settings, Delete, Close }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryFooterButton {
    pub action: MemoryFooterAction,
    pub label: &'static str,
    pub enabled: bool,
    pub visible: bool,
}
impl MemoryUiState {
    pub fn footer_buttons(&self, feedback: bool) -> [MemoryFooterButton; 4];
    pub fn footer_action(&self, feedback: bool, slot: usize) -> Option<MemoryFooterAction>;
}
```

slot 0–3 与 native 控件 7–10、`memory_layout.buttons` 一一对应。反馈 slot 1 固定 retry（无 pending 则不可用），slot 2 固定 Settings；实际管理 slot 2 仍为 Delete。footer_action 对隐藏/禁用/越界返回 None，返回 owned Copy action，native 在释放借用后执行同步窗口调用或原 `retry()`。Task 4 必须保留两个独立反馈动作，不能重新将 pending 与唯一设置入口合并。

`navigation_draft_for` 的签名不变，合同从“只在无 pending 恢复”扩展为上述限定的未接受失败反馈情形。`MemorySnapshot`、`MemoryRecord`、公开 native show/feedback bool 返回、设置返回目的地、Command、receipt、actor human_seen/签名/来源所见版本守卫均没有新增差异。

自审：Settings 路由只存 navigation_draft 和关闭/返回请求，不调用 submit，不改 pending/raw/pending_save/semantic_admitted。Retry 先取 owned command、结束 RefMut 再 submit；footer/focus/route 先取 Copy/owned 模型后才 SetWindowText/EnableWindow/ShowWindow/SetFocus/SendMessage。pending fallback 的 command/raw 为独立块 clone，恢复控件时没有 RefCell guard。合格 draft 优先于 fallback；已接受或不匹配目标的路径仍按原 pending 恢复，不弱化对象身份。

本轮修改只包括 `src/organizer_ui.rs`、`src/platform/windows/harness_memory_ui.rs`、`tests/organizer_ui.rs` 与本报告。未改其他 Task2/Task3/Task4 模块；不派子代理/reviewer，不访问真实 API/密钥/用户数据/屏幕/剪贴板，不执行 full/Release/Node suite。Windows 实机边界仍保留，独立复审由 root 发起。

本轮最终严格检查（最后 40 项源码）：

- `cargo clippy --lib --test organizer_ui -- -D warnings`：exit **0**，`Finished dev profile ... 2.87s`，无警告。首次针对 39 项的严格检查仅报测试断言 `cloned_ref_to_slice_refs`，改为 `sources.as_slice()`/`std::slice::from_ref` 后通过；该 lint exit 101 不是行为 RED。
- `cargo clippy --target x86_64-pc-windows-gnu --all-targets -- -D warnings`：exit **0**，`Finished dev profile ... 10.66s`，无警告。
- `cargo check --target x86_64-pc-windows-gnu --all-targets`：exit **0**，`Finished dev profile ... 8.33s`，无警告。
- `cargo fmt --all -- --check`：exit **0**，无输出。
- `git diff --check`：exit **0**，无输出。

以上是最新草稿/重试说明/native 优先级代码的结果；39 项阶段的 host/Win 检查和初次报告的 105 unique 不能代替这些最终证据。本轮只执行上述受影响范围，无其他 suite。待 root 独立复核 I1 与本 fix diff，未宣称复审已通过。
