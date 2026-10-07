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
