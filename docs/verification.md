# 2026-10-07 交互清晰化与统一设置中心（当前交付）

本轮集成 UX01–UX22，主面板、托盘、功能搜索“设置”进入同一 Win32 原生中心。六组为常规与快捷键、剪贴板与数据、常用路径、智能查找、AI 管家、服务连接。保留原生主题/Segoe UI、分组返回上下文、提交失败 raw 草稿、记忆精确目标与独立重试；截图/建立索引等立即操作保留在独立功能入口。

- 设置只读显示消费 RAM；未加载不显示为 0，配置完整不代表连接成功。不存在设置导航触发的事务、正文扫描或网络请求。用户原已启动的索引终态保留既有一次分块状态校验。
- 数据页显示实际未固定历史保留值，默认 30 天未使用 / 最多 10000 条；可输入 1–36500 天、1–1000000 条。减少限额按原 SQLite 策略清理，固定/收藏豁免；未编辑的非常规旧值不被其他设置重写。
- 三个热键集中且互相冲突会保留旧绑定。Claude/语义开关、首选与索引范围只由智能查找页维护；服务页管理共用 Claude 与独立 Embedding 连接。管家核心与高级调度分别合并当前配置，不覆盖别组设置。
- 原生中心在窄屏使用顶部六组选项，详情可键盘滚动，打开/返回动作固定；生产 geometry 在 96/144/192 DPI 和 360×480、800×600 客户区验证可达。叶子固定动作复用相同几何，长标签换行，连接状态可滚动阅读。
- 设置提交只说明“更改已接收”，不把不关联的旧 Saved 当作本次落盘。后台错误在中心显示；正常退出的保存失败重试合同保持。

## Task 4 gate 修复 round 1

本轮修复路径叶子及其异步保存/清理通知提前发起面板检索、跨管理页丢失原设置来源，以及宽96DPI→窄192DPI叶子说明裁切三项。路径提交命令策略只返回实际 SaveSettings / 明确 ClearPathActivity；导航栈同时保存各层分组与记忆/建议来源，后台 Changed / PathActivityChanged / Saved 经同一门禁延迟面板检索。已启动索引的终态刷新许可不扩张也不移除。

原生创建与 WM_DPICHANGED 共用 EditorSpec 客户区布局，所有字段/checkbox/保留说明/测试状态重新计算宽高与纵向位置；滚动用完整内容坐标定位，保留原 Edit、raw、IME 和焦点，并按新范围约束滚动。宽→窄及焦点/末尾说明回归消费同一生产策略。Windows 实际 GUI 验证仍为0。

修后完整共享测试 `cargo test --locked -- --test-threads=1`：**499 passed / 0 failed / 2 既有 ignored，40 summaries，exit0，无 warning**（151.591s）。集中回归81项及真实actor通知回归1项通过。全套后仅补Win-only初始化期间DPI重排守卫，最终Windows检查/Release覆盖；共享策略与测试未再变化。下方493项和旧产物为 gate 前历史证据。

修后最终host/Windows all-targets strict Clippy/check、fmt/diff均exit0。Windows Release exit0（44.466s）；共享Release core-check exit0（34.494s），内存可见、保存前磁盘为空、退出恢复三项true。所有Cargo串行、固定环境和本机Node路径不变。fresh ZIP采用本轮最终源码/文档及原已核验Node/bundle，准确字节/SHA/CRC和成员以外置manifest/SHA256SUMS及修后实施报告为准。

## Gate 前验证证据（历史）

最终共享全套 `cargo test --locked -- --test-threads=1`：**493 passed / 0 failed / 2 既有 ignored，40 个 suite summaries，exit0**。全套后仅缩短三个 Windows 原生按钮标签，未更改共享行为；最终 Windows 检查与 Release 覆盖这些标签。完整逐 suite 结果见本轮 Task 4 实施报告及保留的验证日志。Windows GUI、中文 IME、多屏 DPI、WinHTTP/DPAPI/通知实际执行数 **0**；macOS 共享/合成进程测试和 Windows 交叉编译不能替代实机视觉验收。未使用真实 API、用户密钥、数据库、剪贴板或屏幕。

最终串行检查全部 exit0：host/Windows `cargo clippy --locked --all-targets [--target x86_64-pc-windows-gnu] -- -D warnings`、对应 all-targets `cargo check --locked`、`cargo fmt --all -- --check`、`git diff --check`。Windows Release `cargo build --release --locked --target x86_64-pc-windows-gnu` 通过（46.017s）；共享 Release `cargo run --release --locked -- --core-check` 通过（34.505s），`pending_visible`、`disk_empty_before_flush`、`exit_restored` 均为 true，SQLite 3.53.2。固定环境为 `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`，所有 Cargo 严格串行。

Node 协议、生产源码与已构建 Pi bundle 未变；本轮不重跑之前通过的 30 项 Node suite。Rust 子进程测试使用固定本机 Node，最终便携包使用本轮已核验 Node 24.19.0 Windows x64 资产。准确成员、字节与 SHA-256 以 ZIP 外的 `dist/package-manifest.json` / `dist/SHA256SUMS` 为准，打包后逐成员核对 source/stage/ZIP 与 CRC，不把校验清单打入 ZIP 自循环。

## 以下为此前 Pi 运行时版本的验证记录

以下测试数与旧产物只标识此前版本，当前交付以本节及外置清单为准。

# 2026-10-07 Pi 整理运行时验证

当前交付物的精确体积、成员及 SHA-256 以 `dist/package-manifest.json` 与 `dist/SHA256SUMS` 为准；清单位于 ZIP 外，避免自哈希循环。ZIP 包含 QuickPanel.exe、固定 Node 24.19.0、Pi bundle、全部相关许可及本次使用说明。

本次验证为 macOS 共享核心、真实 Node/Pi 合成进程与消息桥；Windows 原生窗口、WinHTTP/DPAPI/Job fixtures 只交叉编译，执行数为 0。未调用收费模型 API、读取用户密钥/正文/剪贴板或截屏。最终共享全套457项通过、0失败、2项原有忽略，38个suite summaries且无warning；Node真实Pi suite 30/30通过。

## 本轮结果与边界

- Rust共享核心：457 passed / 0 failed / 2 原有 ignored。先前一次full被旧6字段fixture阻断，修正为8字段后完成；另一次因focused输出检查失误中断，所有日志保留于本轮开发报告，没有计作通过。
- 真实进程/通道：14个worker测试覆盖默认关闭无spawn、无模型health、真实Pi→actor→完成回执、EOF/坏帧/有界stdout和stderr、迟到token/call、取消不释放真实占用、deadline中断不读stdin的大帧、最终checkpoint先于失败并保留elapsed、panic和owned PID清理。
- Memory UI便携行为：所见快照、Attention不可应用、同id/input保存重试、多行输入归一化/原草稿保留；真实SQLite标签投影只读以及跨启动新操作id验证。
- Host与Windows all-targets严格Clippy/check通过；fmt/diff通过。Windows-only fixtures只编译，未在开发主机执行。新增WinHTTP localhost fixture验证真实序列化请求与signed/redacted thinking的规范化，只列编译证据。
- Windows GNU Release及共享Release core-check通过；core-check的内存可查、保存前磁盘无历史、退出恢复三个断言均true。
- 最终全套已纳入本批全部 Rust 修改，包括保存重试、空 IME 组合、反馈长度、显式滚动坐标及原 Windows 最小缩窗保护。全套之后生产源码冻结；Windows-only fixtures 仅编译，不能用宿主 0 项运行声称实机通过。
- 固定Node24.19.0/pnpm11.25.0、禁用生命周期脚本的frozen安装后重新构建Pi bundle，212602bytes。用户启动不下载，不需要全局Node。
- 本机5次拥有的ready-only子进程：启动47.198–51.665ms，RSS采样最大值55,920–56,112KiB（约54.61–54.80MiB）。不含模型，不是Windows测试，也不代表实际峰值。
- 全新ZIP含EXE、Node、Pi模块、Node/Pi/TypeBox及原有依赖许可、最新README和docs。逐成员精确集合/字节/CRC/SHA校验；最终EXE/ZIP/每个成员哈希在ZIP外的dist/SHA256SUMS及package-manifest.json。

需要Windows实机继续验收原生窗口、中文输入法、多屏DPI/小屏布局、WinHTTP/DPAPI与Job对象生命周期；开发过程未抓屏或操作用户剪贴板。CI准备步骤已更新，但本轮未触发CI或推送。

## 最终审查修复

- 保存重试按同一 pending 请求记录首次 RAM 接纳，并绑定发往 actor 时的旧 session token；只有首次且 token 匹配才取消旧 worker。重复保存成功或再次失败都保留后来启动的任务。真实 actor、SQLite 失败触发器、Pi 子进程和生产 portable policy 覆盖反馈/编辑/删除 × 重试成功/再失败六流程，校验新任务预算不变、正常完成、无重复记忆、下一批继续。
- 反馈窗口以 WM_IME_STARTCOMPOSITION/ENDCOMPOSITION 生命周期配合当前组合串检测保护 Esc/Enter/Tab，空组合期间继续交给输入法。便携策略已执行，中文输入法实际效果仍待 Windows 验收。
- 反馈原因统一为 1–256 字符，记忆规则仍为 1–512 字符；换行/制表符在提交时转为空格，失败保留 raw 输入，路径/别名不采用此归一化。256/257 与 512/513 边界已验证。
- 内容 viewport 按共享 geometry 减 scroll_y 显式批量定位全部子控件，使用 DeferWindowPos（失败回退 SetWindowPos），不再依赖 SW_SCROLLCHILDREN。部分裁剪坐标、原 18 组小屏/DPI geometry、正反向 Tab/resize clamp 已验证；实际 Win32 重绘仍待验收。
- fresh 打包仅清理废弃生成文件 dist/SHA256SUMS.txt，先校验 owned path；实际测试确认软链接拒绝且目标保留、其他 dist 文件保持。最终校验仅使用 dist/SHA256SUMS，全部 23 成员再次核对 source/stage/ZIP、CRC、体积与 SHA。
- 本批 focused 80/80（14 worker + 32 portable UI + 2 coordinator + 32 actor）；source-final full 457/0/2，所有生产修改已包含。Node 源码与 bundle 未变，沿用此前 30/30 真实 Pi suite，不重复未变化的套件。

## 最终审查与交付收尾

最终产品源码为 `d0dd6e47b6366c4a49dae1af2a9f691ba9d20625`。一次全功能审查提出的保存重试、空 IME 组合、反馈长度、旧校验文件和部分子控件重绘五项，在同一修复提交后经一次范围复审全部关闭；未发现修复引入的新 Critical、Important 或 Minor。Windows 实机验收边界仍保留。

主控独立读取最终全套日志，汇总为 457 passed / 0 failed / 2 既有 ignored、38 个 suite summaries、无 warning，并核对最终严格检查、Release 与退出保存 core-check 三项 true。交付 ZIP 的全部 23 个成员逐字节核对当前 source、staging 与归档，CRC、SHA-256 和外置唯一清单均一致。补充本段后仅重新打包和核对文档成员，没有变更产品源码或重复运行测试。

计划、过程记录、三个实施报告、审查与复审、RED/GREEN、构建及独立校验证据保留在项目的 `target/verification-2026-10-07-pi-harness/`；运行时二进制已保存在便携包内。本轮五项实施取舍及代价见 [设计中的实施取舍](superpowers/specs/2026-10-07-pi-harness-design.md#实施取舍)。代码和交付物继续保留在原目录与 `codex/quick-panel` 分支。

## 本轮 Task 3 审查修复

Task 3 的451项全套包含以下回归（亦保留在最终457项中），Node生产协议与bundle未改，沿用已通过的30项Node验证：

- stdout reader独立合并通过协议校验且关联当前request的checkpoint/typed失败，HTTP阻塞时也不会丢失已接收的最终预算。真实Pi恢复elapsed119750并阻塞transport跨剩余时限，最终cp120000先于HarnessFailed(deadline,false)，后到的网络错误不覆盖终态。陌生request与未settled transcript拒绝，不进入actor。
- 记忆/反馈窗口新增内容级viewport、scrollbar/wheel、Tab焦点自动滚入视野；底部保存/重试/删除/关闭独立固定，窄窗口使用两行。共享geometry测试覆盖96/144/192 DPI、三种小屏客户区、记忆及反馈两模式（18组），逐字段无重叠、可完全滚入、footer不遮挡；另覆盖反向Tab和resize后的scroll clamp。原ListBox/编辑器自身滚动、raw草稿、IME、theme/DPI字体保持。
- 本轮focused 13个worker + 27个portable UI测试全部通过。Windows GUI仍只交叉编译，没有截图或实机视觉验收；原有447项版本的失败/中断证据与本轮RED/GREEN日志分开保留。

## 以下为历史版本的验证记录

以下日期、测试数和旧 EXE 哈希只标识当时版本，不标识本轮产物。

# 验证记录

当前交付为单条 AI 整理状态补充，日期 2026-10-06；产品源码 `c281c12375cd16a10a88fe84d5ccab10606a7ccd`，之后只更新验证文档。代码与交付继续保留在原项目目录和 `codex/quick-panel` 分支。macOS aarch64 宿主，Windows 使用 `x86_64-pc-windows-gnu` 交叉构建。

## 单条整理状态与当前交付

主列表只给实际 Queued、Processing、Ready 来源显示“待整理”“整理中”“有整理建议”；关闭、范围外、未入队旧内容及 Done 不标记。原名称、正文、执行目标和本地路径区分保持可用；确认前不采用建议别名。启用时有可点击的 AI 管家入口，统一显示冷却、暂停、容量或建议数量，无逐行报错、估算百分比、动画和新增快捷键。

轻量状态命令只投影最多 256 条内存记录，不读取正文/SQLite、不执行 reconciliation、不标记 dirty。一个实际在途请求加代次合并，隐藏/配置变化前的回包不能恢复旧标记；显示缓存按 revision、结果与查询复用，不在绘制时查询后台。AI 元数据变更以带前值守卫的 alias/tags 补丁更新现有行，不触发搜索；已在途结果保留已有行的相对顺序并追加新匹配项，下一次搜索恢复正常排序。原生只替换变化行的辅助文字并保留选择/顶部位置，界面编辑内容不被重设。

独立审查确认两项边界问题并修复：冷却自然到期后仍显示 cooling，以及同查询晚到结果间接重载当前预览。修后按当前时间投影冷却，保留原调度/失败计数且不安排保存；晚到结果仅在来源、目标、类型和标题仍一致时保留原预览，实际选中身份再次校验，用户显式选择/重试仍沿原路径。两项均经过真实 RED→GREEN 与定点复审，无开放发现。

| 当前检查 | 结果 |
|---|---|
| 修后完整共享测试 | **389 passed / 0 failed / 2 既有 ignored**，35 个 suite summaries |
| 初版 focused / 修复 focused | 101 / 69 项通过；容量用例均在最终 full 中执行 |
| host / Windows all-target strict Clippy | 最终源码均通过，`-D warnings`；Windows 38 个专用 fixtures 仅编译、0 执行 |
| fmt / diff check | 通过 |
| Windows GNU Release | 最终源码构建通过，30.73s；PE32+ GUI x64，4,734,976 字节，只导入系统 DLL / CRT |
| 最终共享 `--core-check` | 内存可查、保存前磁盘无历史、正常退出可恢复，三项均为 true |
| 独立审查 | `70fe96a..fcf8f83` 初审；`fcf8f83..c281c12` 定点复审通过 |

最终 EXE SHA-256：`3dd0b33b9852d355c56f956f7686d0bfe071bf3bba18e2aa5a60c60424a3d9d3`。新建 ZIP 打包当前 EXE、README、许可与 docs，校验 CRC 和全部成员字节；当前 ZIP/EXE 校验值位于 `dist/SHA256SUMS.txt`。

本轮在原 SSD 工作区执行，磁盘空间已充足，不使用 RAM 副本或迁移缓存。固定环境 `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`；Cargo 串行。最终完整命令为 `cargo test --locked`，编译为 `cargo build --locked --release --target x86_64-pc-windows-gnu`。证据与两轮审查记录保留 `target/verification-2026-10-06-organizer-status/`；较早 `fcf8f83` 的 387 项结果不冒充最后源码结果。

新增回归覆盖真实 actor 的入队/处理中/Ready/Done、范围与关闭、原内容可用、状态读不提交事务，以及旧回包、人工前值保护、晚到结果保序、冷却到期和预览来源保护。没有使用真实 API、密钥、剪贴板或屏幕；未重测性能基准。Windows 原生界面、滚动/文字选择、Tab/Enter、DPI/主题及实际模型效果仍需实机验收，交叉编译不代表这些交互已执行。

## AI 管家基础交付（历史验证 `1bb5874`）

AI 管家基础集成验证日期：2026-10-06；该阶段产品源码为 `1bb5874f95424e094676f95b276cb1d45b8b858f`。此前外部路径与截图基线为 `fb9d8b1`，对应旧记录保留在后文。验证宿主为 macOS aarch64，Windows 包通过 `x86_64-pc-windows-gnu` 交叉构建。SQLite 为随程序编译的 3.53.2，sqlite-vec 固定为 0.1.9（运行时 `vec_version()` 为 `v0.1.9`）。

共享核心和 Windows 源码编译通过；Windows 原生界面的实际运行尚未在本机验证。最终源码的验证结果如下，后文较早阶段的测试数量不代表当前版本：

| 最终检查 | 结果 |
|---|---|
| 完整共享测试 | **379 项通过、0 失败、2 项既有重型测试忽略**；34 个 suite summaries |
| macOS / Windows all-target strict Clippy | 两者通过，`-D warnings`；Windows all-target check 通过 |
| fmt / diff check | 通过 |
| Windows GNU Release | PE32+ GUI x64，4,692,480 字节；只导入 Windows 系统 DLL / CRT |
| 共享 `--core-check` | `pending_visible`、`disk_empty_before_flush`、`exit_restored` 均为 true |
| Windows-only fixtures | **38 项仅交叉编译、0 项执行**：AI 22、Embedding 8、images 8 |

该历史版本 EXE 的 SHA-256 为 `464e0d95fab8c3aebc002c419c2a48affb3c0854d021bff2acf03535b07880cd`；当前版本以文档开头和 `dist/SHA256SUMS.txt` 的校验值为准。

## AI 管家、模型与原生审阅（2026-10-06）

[设计](superpowers/specs/2026-10-06-ai-organizer-design.md)；[实施计划](superpowers/plans/2026-10-06-ai-organizer.md)。Task 3 基线 `c2b84d7`，包含 Task 1/2 已审查实现和源保留路径标签 `2790380`。本节记录本轮集成证据，不把 macOS 交叉编译当作 Windows 执行。

原生 AI 设置提供可输入模型 ID 的下拉框、异步获取模型与现有连接测试，以及默认/low/medium/high/xhigh/max 强度。列表、测试均使用当前未保存配置，能力缓存随地址/认证/密钥变化失效。缺 capabilities 是未知，只有明确 reported unsupported 才本地提醒；默认完全省略 effort，不静默降级重试。共享单槽设置 worker 在取消后保持实际占用，关闭和重开不能叠加请求；panic 有终态回执。

管家设置与检索开关独立，缺接口配置在本地设置和管家状态说明；路径范围提示独立观察开关，不自动开启监控。后台 timer 只在启用且本地配置有效时创建，间隔为用户选择的 30–3600 秒。原生管家 worker 惰性启动，一次一个真实请求、无待发网络队列，取消保留物理占用直到结束，不 join。Work 使用派发时的精确 AI/管家快照，结果仍由 actor 验证 token、配置和来源版本。

审阅窗口读取已有本地建议，真实 ListView 复选框支持多条应用/忽略，原名/别名/标签/依据与可选文字的上下文在页内展示，选中项可编辑。一个当前 Snapshot、一个未完成 Snapshot 请求；编辑绑定 expected_version，不自动转移到新版本。来源/保存错误保留编辑直到显式刷新，存储失败保留 pending save。只认 `OrganizerReceipt.request_id` 对应的事务/保存屏障，旧 Saved 或 RAM Ready 消失不会显示本次成功。只读 `OrganizerDetail` 使用来源与版本守卫另取完整原样 target，最多 32767 UTF-16 单元，一个未完成详情请求，切换选择/闭窗 epoch 拒旧结果，不读 BLOB/文件系统、不发网络、不写盘。

视觉沿用 theme::Colors、Segoe UI 和实际 DPI 字体。长设置表单使用原生滚动 viewport、固定保存/取消按钮及焦点滚动，高 DPI 不通过缩小字体隐藏字段。审阅窗口采用顶部状态、中部真实列表、下部可选择详情与编辑区、底部明确动作；复用明暗主题和窗口工作区边界。界面截图、100/150/200% DPI 鼠标与键盘/IME、焦点、窗口实际关闭/重开仍需 Windows 实机验收。

| Task 3 初版阶段检查（历史记录，`a9aa6d2`） | 结果 |
|---|---|
| 最终 focused（跳过两项此前已验证的大容量用例） | **75 passed / 0 failed**：lib 8、连接生命周期 6、organizer actor 21、UI 11、palette 22、path_names 7 |
| 一次完整共享 suite | **363 passed / 0 failed / 2 项既有 ignored**，33 个 suite summaries；organizer actor 23 项含两项容量测试 |
| macOS / Windows all-target strict Clippy | 两者 `-D warnings` 通过 |
| Windows fixtures cross-check | **38 项只编译、0 项执行**：AI 22、Embedding 8、images 8 |
| fmt / diff check | 通过；交付前再次检查 |
| Windows GNU Release | 通过，优化构建 1m 50s；可执行文件保留 RAM target 后由主控同步 |

真实 RED 包括：缺便携解析/生命周期模块；取消/关闭状态槽过早释放；新版本错误接收旧编辑；worker panic 无终态；回执接口缺失；混合 stale Dismiss 被当作成功；容量释放后 saturated 粘滞；完整原路径接口/详情 gate 缺失；关闭 AI 时管家入口不可查的目录回归。SQLite 回执测试构造真实 UPDATE 失败触发器，验证磁盘别名/Ready 同时保持旧值、RAM Ready 已移除仍明确失败、后续关联 SavePending 确认实际提交。详情测试验证超出 1024 摘要的原大小写 target、PRAGMA data_version 不变和改源版本拒绝。

构建在 `/Volumes/QPPathNamesBuild/workspace/quick-panel` RAM 副本执行，环境统一为 `TMPDIR=/Volumes/QPPathNamesBuild/tmp CARGO_TARGET_DIR=/Volumes/QPPathNamesBuild/target CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`。原则上串行 Cargo，后文保留一次短暂重叠记录。最终日志与审查记录另存回原项目 `target/verification-2026-10-06-ai-organizer/`，RAM 路径仅描述构建时环境。共享 SQLite/线程用例是功能证据，不证明 SSD 耐久性、Windows 性能或模型语义效果。未访问用户真实密钥、API、剪贴板、屏幕或文件正文；实施代理未继续派生代理，主控组织了独立审查。

Task 3 定点修复第 1 轮（基于 `a9aa6d2`）：原样保存 AI/管家设置不再取消实际在途结果；UI session 与 actor 使用同一相关配置比较，真正变更仍撤销且不提前释放网络占用。保存回执记录本批来源，只清本次提交的草稿和勾选，SavePending 延续同一来源集合。编辑框保存原始别名/标签文本，仅提交时规范化；同 source/version 的后台刷新不替换输入，必要替换等待 IME 结束，输入仍绑定实际显示的来源版本。

本轮 focused 为 **25 passed / 0 failed**（organizer_ui 17、connection_test 6、真实 worker/actor coordinator 2）；先观察原样保存丢回执、未选中 A 草稿丢失、raw 尾逗号/空白消失等真实 RED，再修至 GREEN。IME 同时使用 Edit 子类的 Start/End 状态与 GCS_COMPSTR，尚未产生首个组合字符时也不替换或禁用编辑框。macOS/Windows all-target strict Clippy 和 Windows all-target cross-check 通过，最终 source 后更新 Windows Release。该轮未重复 363 项整套；日志为 `task3-fix1-*.log`。该轮暂缓的 ListView 阅读滚动与持续成功消息遮挡状态两条 Minor 已在下面的最终修复中处理。Windows 实机 IME/光标/选择验收仍未执行，门控共享测试与交叉编译不能替代它。

最终清单修复（基于 `0d695d6`）：actor 派发前使用与生产 `prepare` 共用的真实 Messages body 编码检查 64 KiB 上限，再预留既有 store/未来 Ready 的 2 MiB 预算。保留最大可发送前缀，其余保持 Queued；没有 worker 内隐藏请求队列，本地构造错误不发布失败 Work、不计接口冷却、不影响普通搜索。无效地址/模型沿既有管家配置提示处理，保留配置及来源。真实 Engine + `prepare` 的中文、emoji、引号和反斜杠 fixture 验证每个发布批次都可发送、余项继续、全部来源保留且 failures 始终为 0。单项全字段最大值三种转义极限真实 body 均为 **19,203 bytes**；另覆盖多行控制字符的合法转义及无效边界，非法整理元数据仍保留在主历史且不阻塞后续合法来源。

原生审阅缓存仅保存最多 128 个展示行的 source/名称/别名/标签投影，详情、状态、checkbox 不触发行重建；必须重建时按稳定 source 恢复顶部与记录的像素偏移，来源被删除时优先后邻、再前邻。选择、checkbox、raw 草稿及 IME 延迟门控保留。成功回执与实时排队/处理中/冷却/暂停/容量状态并列，未保存及保存中提示优先，pending-save 和草稿守卫不因状态显示而解除。可移植测试覆盖行更新判定、插入/删除/显示变化后的锚点与偏移、成功后的五种实时状态及写失败保护；Windows 接线通过交叉编译，真实滚动尚未执行。

最终 focused：**82 passed / 0 failed**（lib 8、Claude 17、organizer actor 24、protocol 12、UI 21，2 项容量用例留给最后 full）。host/Windows `cargo clippy --locked [--target x86_64-pc-windows-gnu] --all-targets -- -D warnings` 与 Windows `cargo check --locked --target x86_64-pc-windows-gnu --all-targets` 均通过；日志为 `/Volumes/QPPathNamesBuild/tmp/final-fix-*.log`。执行纪律记录：启动 host Clippy 时误把仍有 session-id 的最后 focused poll 当作完成，两条 Cargo 短暂重叠，均成功且无锁/编译故障；后续 Cargo 均等待明确 exit 后再启动。

唯一一次最终 `cargo test --locked`：**379 passed / 0 failed / 2 既有 ignored**，34 个 suite summaries；全部 26 个 organizer actor 用例通过（45.04s），含两项既有缓存/Ready 大容量用例，最后代码下的 2 MiB 内存与未来结果预留仍成立。日志 `final-fix-full.log`。Windows all-target 编译涵盖现有 38 个 native fixtures，它们在 macOS 上执行 0 项。

最后源码后的唯一一次 Windows GNU Release 构建通过（30.02s，`final-fix-release.log`）；`cargo fmt --all -- --check` 与 `git diff --check` 最终通过。主控读取完整日志，独立汇总为 379/0/2，并检查 EXE 类型、系统 DLL 导入及哈希。随后在相同源码运行 `cargo run --locked -- --core-check`，三个内存优先/退出恢复断言均为 true；使用合成临时 SQLite，无用户数据。最终 EXE 复制到原项目的 `target/x86_64-pc-windows-gnu/release/quick-panel.exe` 和 `dist/QuickPanel-Windows-x64/QuickPanel.exe`。

三个任务的独立审查及必要定点修复均完成。最终全范围审查发现的请求尺寸预算、列表滚动、成功消息遮挡状态三项问题，在唯一修复波 `0d695d6..1bb5874` 中全部处理；定点复审逐项确认 ADDRESSED，修复 diff 新问题与范围外观察均为 None，未解决发现为 0。没有重复执行不变代码的完整 suite，也没有将交叉编译或协议 fixture 当作真实接口/Windows GUI 验收。

## AI 管家实施决定

以下按本轮 `Ruling:` 的记录顺序保留全部三项决定与代价：

1. **AI 检索与管家独立启用，共用接口、密钥、模型和推理强度。** 用户可以只开后台整理，界面继续使用“启用 AI 按意思查找”的精确名称，并在管家页说明独立关系。代价是关闭检索不等于关闭全部 AI；若希望停止全部 AI，需同时关闭管家。
2. **磁盘耗尽时只迁移本项目可重建产物与可下载 Cargo 缓存。** 本项目 target、94 个下载归档及后续 5 个已解压依赖目录经字节核对后暂存 RAM 并原位链接；空间恢复后均已还原为普通目录/文件，恢复义务已履行。另清理 673 个已有解压源码的公共 crates.io 下载归档（113,966,305 字节），保留解压源码、其他注册表和用户数据。代价是未恢复链接会造成断链，已核查移除；删除的公共下载归档未来可能需重新下载。相关清单保存在本轮验证记录目录。
3. **磁盘仍满时使用完整 RAM Git 开发副本，保留原目录中的已有改动。** 以原 HEAD 和文件哈希为基线，将新提交历史与提交中的文件安全同步回 `/Users/chuwujie/workspace/quick-panel`，碰到未预期的本地变更即停止覆盖。代价是同步前 RAM 工作不可视为持久交付；源码、提交、验证证据和产物必须全部回存并校验后才卸载临时卷。最终保留原 `codex/quick-panel` 分支。

## 外部常用路径与区域截图（历史验证 `fb9d8b1`）

[本轮设计与边界](superpowers/specs/2026-10-05-external-paths-and-screenshots-design.md)；[实施计划](superpowers/plans/2026-10-05-external-paths-and-screenshots.md)。

外部路径推荐主要读取前台 Windows 资源管理器的活动可见文件系统视图，无须先通过本工具打开或收藏。入口为“常用推荐设置”，也可从设置/托盘菜单进入；默认关闭，用户启用后每两秒探测，路径稳定至少1.5秒才记录。同一窗口停留/重新聚焦不重复计次，规范路径30秒内跨来源合并。Windows11共享顶层窗口的标签页以活动视图COM身份、可见性、前台root及PIDL复核；无法确定的分支跳过。第三方文件管理器、虚拟位置和后台未访问的标签页不承诺覆盖。

Recent作为可单独关闭的文档补充，依赖Windows和应用写入最近文档；带启动参数、参数读取失败或无法确定的链接跳过。只读取Recent目录和链接自身，不Resolve、不探测目标存在性、不读取文档正文；首次及故障重建仅建立基线。基线最多2048条、单次目录枚举最多8192条、链接最多1MiB、每轮解析最多16条变化，多余变化跳过；超限或不完整基线不回放。通知初始化或续订失败至少间隔30秒才重建，复用同一个实际worker，保持Explorer来源继续工作。以上是有界访问观察，不是完整文件打开审计。

排名在本机按7天半衰期衰减的访问权重计算，Explorer权重1、Recent权重0.5。最多1024个有效路径、128个“不再推荐”项，路径最多2048UTF-16单元，核心路径缓存预算2MiB，独立观察队列预算256KiB。90天未访问的非抑制记录在查询中隐藏，并在有变更的保存事务中清理；不清理人工收藏或“不再推荐”偏好。大小写折叠的身份key与保留大小写的实际目标分开，避免改变可打开的路径。预算不是整个进程RSS承诺。

自动路径以独立条目加入三种检索的本地结果，空查询最多5条、有输入最多10条，总列表最多50条。与同请求当前结果中的人工收藏按规范路径去重；若Semantic结果未返回某收藏，可出现同路径自动项。自动项支持打开、定位、复制路径、明确收藏和不再推荐；最近访问时间与原有保存时间分开。设置支持排除目录、关闭和清空，暂停剪贴板记录与路径开关独立。

自动统计不插入items/FTS/向量表，不发送给AI，不自动成为收藏；明确收藏后遵循原来的索引规则。变化路径与已有历史在同一个约55秒开始、健康情况下60秒窗口的事务中提交；查询不创建表、不写盘，失败保留待保存状态，退出保存已接收数据。纯路径变更不重启语义来源/状态分页；如果同次维护真正删除了过期正文和向量，实际效果仍会使语义状态失效。

区域截图从功能搜索、设置菜单或托盘进入，拖选后松开鼠标确认；截图快捷键有独立小设置窗，默认关闭。三组热键保留交换、冲突回滚及旧配置兼容。原生选区采用物理坐标并处理负原点、多屏和DPI；取消、失去捕获、显示变化或退出不提交旧意图。截图自身隐藏与失活不会误撤销，结束后不抢用户已切换的前台。

选区层隐藏/销毁后只读取确认区域，不持续抓屏、不缓存整个桌面、不生成临时图片。原始BGRA预算64MiB；后台WIC编码设置不透明alpha并确认实际像素格式，最终PNG预算8MiB，按预算准备兼容DIB。一次真实编码在途，所有失败/取消/退出路径有终态。发布先完成分配，再在OpenClipboard锁内复核意图和原剪贴板序号；期间用户新复制的内容不会被覆盖。成功后的准确自写序号抑制监听回声，允许记录时仅入队一次PNG，兼容位图不持久化。暂停剪贴板记录仍允许复制截图，源应用排除规则按真实剪贴板owner进程名执行。

原始像素、编码stream、GDI和兼容表示各有内存开销，上述预算不是进程总RSS承诺。截图反映实际读取时的屏幕，选区期间内容可以变化。EmptyClipboard之后若系统SetClipboardData失败，无法保证恢复原内容；代码明确区分PNG未写入、仅兼容位图失败，以及已复制但历史未入队。

三个任务的专项审查与复审、最终全范围审查及定点复审均已完成，开放finding为0。修复了执行路径大小写、90天清理、真实保留删除的语义状态更新、纯路径变更误重启状态扫描，以及Recent通知恢复和带启动参数的链接准入。Recent链接只有明确读到完整空参数才接受；非空、失败或不确定回执跳过。

| 当前检查 | 结果 |
|---|---|
| 全套共享测试 | **301 项通过，0 失败，2 项既有重型测试默认忽略**；最后修复后完整重跑 |
| fmt / diff check | 通过 |
| macOS / Windows all-target strict Clippy | 最终源码两者通过，`-D warnings` |
| Windows GNU Release | 最终源码构建成功；PE32+ GUI x86-64，仅系统 DLL / CRT 导入 |
| 当前共享core-check | 内存可查、保存前磁盘无历史、正常退出后恢复，三项均为 true |
| Windows-only fixtures | **32 项仅交叉编译，0 项执行**；Claude 16、Embedding 8、images 8（含3项新截图内存fixture） |
| 独立审查 | 全部通过；最终修复范围 `47ed667..fb9d8b1`，无未解决发现 |

测试使用本次创建的2GiB HFS+ RAM卷，TMPDIR=/Volumes/QPFeatureBuild/tmp、CARGO_TARGET_DIR=/Volumes/QPFeatureBuild/target，并禁用incremental、串行Cargo、关闭dev/test debug信息。源码、最终可执行文件与ZIP保留在项目目录；临时卷按设备UUID核对后清理。共享SQLite/线程测试证明功能行为，不能作为SSD耐久性、Windows性能或原生UI运行的证据。本轮没有重跑大型基准，也没有访问用户真实Shell/目录/剪贴板/屏幕、调用付费服务或使用用户密钥。

Windows原生Shell、Recent通知、图片编解码、截图选区、多屏DPI、快捷键及跨应用粘贴仍待实机验收；交叉编译不等同这些行为已执行。

最终完整命令：`TMPDIR=/Volumes/QPFeatureBuild/tmp CARGO_TARGET_DIR=/Volumes/QPFeatureBuild/target CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --locked -- --test-threads=1`。此后重新构建Windows Release，并再次运行共享core-check；最终产品源码 `fb9d8b1`，后续仅整理验证文档。

## 外部路径与截图的实施决定

按作出决定的顺序保留范围、原因及取舍：

1. 用户已提出两项功能并澄清外部文件夹优先，按上一轮具体建议和本轮来源修正继续实现，不重复确认执行方式 — 授权范围足够且无外部发布 — 原生覆盖有限需如实交代。
2. 沿用当前用户指定目录与 codex/quick-panel 分支，无remote，不另建工作树 — 延续既有偏好并节省磁盘 — 小提交可回退，保留分支交付。
3. 将两个功能放入同一三任务计划，串行完成共享引擎/原生UI/剪贴板接线 — 共享保存和退出契约需要整体审查 — 单任务仍独立验证，交付用时可能增加。
4. 外部采集默认关闭，由设置启用；资源管理器为主，Recent为补充，第三方应用不承诺全覆盖 — 原生自动化无法可靠审计所有软件 — 用户需开启功能，未观察到的访问不排名。
5. 首版截图交付PNG剪贴板和兼容位图，用户要求的JPG或PNG取PNG；不添加JPG文件导出/标注 — 直接满足粘贴并限制无关扩展 — 如需文件导出再独立增加。
6. Task1新增事件只做最小Windows match兼容，Task2接入展示；SaveStats最小新增semantic_status_changed — 保证中间提交可检查且路径提交不触发全库状态扫描 — 接口扩展需两任务共同审查。
7. 沿用已验证242 pass/2ignored的同源码基线，不在无代码变化时重复全套；每任务focused，Task1和最终portable各一次full — 遵守验证与避免冗余 — 最终集成仍要全套覆盖。
8. New owned 2GiB HFS+ RAM volume /Volumes/QPFeatureBuild, /dev/disk7, UUID 694AD54B-9F61-36EF-A98E-243AF480DDB3; source and dist stay on SSD. TMPDIR=/Volumes/QPFeatureBuild/tmp, CARGO_TARGET_DIR=/Volumes/QPFeatureBuild/target, CARGO_INCREMENTAL=0, CARGO_BUILD_JOBS=1, debug=0. Functional tests are not SSD/Windows performance. Copy needed outputs, verify UUID and detach on finish.
9. QuickPanel观察权重1、既有Used不自动转换；时钟回拨允许重新计次，未来时间截到接收时刻，同批仍去重 — 契约未指定该来源权重且需避免永久冷却 — 人工系统调时可能影响一次计数，统计不是精确审计。
10. 自动记录90天保留期按Global Constraints作为必需实现，设计正文“可清理”只表示允许在有变更事务清理，不表示永久省略 — 长期满额后需腾空间 — 过期自动推荐会消失，人工收藏及不再推荐偏好保留。
11. Task2允许lib.rs最小pubmod声明以暴露新便携observer测试模块 — 计划Create模块但漏列声明文件 — 不改变其他核心接口。
12. Task2 Entry::id保持既有&str；Suggested改为缓存id与PathSuggestion的SuggestedEntry包装，suggested()仍借用原数据 — brief载荷写得过细，直接使用会使所有既有条目每次id访问分配并对推荐反复hash — UI包装多一个小类型，核心公共契约保持不变。Worker已同意并采用；后续审查按此裁定。
13. “与已有收藏去重”解释为当前请求的实际展示集合，规范路径相同则人工Saved优先；不做全库排除 — 避免每次输入额外全库扫描，也保留语义结果未含该收藏时的可用路径 — 同一路径在另一结果模式可显示为自动推荐，README准确说明；收藏仍显式表单确认。
14. Recent通知临时故障同代次每30秒最多重建一次，保持Explorer独立可用并重建基线 — 让可选来源能自行恢复且不紧密重试 — 故障恢复可能延迟30秒，重建期间访问可能漏记，属于补充观察。
15. Recent每轮超16的额外变化丢弃、基线推进，按有界补充来源契约接受并在文档披露 — 主需求为外部文件夹且需限制短时读量 — 批量打开文档时计数不完整，不能称为完整审计。
16. Task3允许lib.rs模块声明与Cargo中WIC编码需要的StructuredStorage feature — 计划声明新portable模块且WIC encoder依赖IPropertyBag2，遗漏入口文件 — 不增加第三方crate或改变原有功能。
17. SSD fell from1.7GiB to116MiB and blocked source writes; moved ONLY rebuildable target/debug (213MiB), target/release(65MiB), target/x86_64-pc-windows-gnu(224MiB) to /Volumes/QPFeatureBuild/ssd-target after byte-for-byte filecmp verification. Kept original locations as symlinks to preserve dependency absolute paths; old top-level target logs/reviews untouched. SSD now578MiB, RAM free907MiB; project write test passed. Before detach, remove ONLY these exact symlinks, recreate local target directories and preserve finalEXE. All source/dist remain SSD.

## 性能、交互与功能优化（历史验证 b667a97）

本轮覆盖审查提出的全部优化：

- 普通、Claude 派生和语义结果查询只保留最新请求；长候选和索引状态扫描分段执行，在检查点响应可靠命令、到期保存和退出。保留完整召回，不用提前截断候选换取速度。
- 索引扫描使用独立源版本，纯向量事务不再使扫描回到起点；固定状态过滤改为集合查询。保留策略和孤立数据维护按实际变更、策略变化和时间条件触发，删除仍在事务内保持一致。
- 路径检查、Shell 操作和图片准备移到有界后台任务。预览仅保留一个真实加载和最新目标，同内容刷新保持选择和滚动位置；失败与找不到条目都有终态，后续操作可重试。文本预览可滚动、选取和复制，显示最多 6000 字符并提示截断，原始文本完整保留。
- 有可用 PNG 时允许跳过超预算冗余 DIB；预览源上限 100 MP，缩略图长边最多 1024、输出最多 4 MiB，可选粘贴兼容 DIB 最多 64 MiB。兼容转换失败不丢失原 PNG；这些预算不代表进程总内存或系统解码器内部内存的上限。
- 主栏统一为一个智能查找入口，设置可选 Auto、Claude、Semantic。Auto 优先使用已启用且有有效向量的语义检索，否则使用已启用 Claude；只有 Embedding 时引导手动建索引。索引状态按需合并并仅缓存在内存，不随每个输入字符扫描；零有效向量不会发送 Embedding 查询。
- 固定剪贴板可编辑名称、别名和标签并即时保存，保持正文、原格式和重新复制后的自定义名称。结果显示来源、命中片段及本地保存时间；外部唤起选中旧查询，内部刷新保留操作状态。主题色和画刷缓存复用。

普通输入仍只执行本地检索。API 失败安静回退，两个设置页的连接测试保留；存储故障继续明确显示。状态回执、后台动作和加载结果按各自请求身份与交互状态校验，失焦或取消后不采用旧检索结果；更换目标后不执行尚未提交的旧动作，已提交操作仍接收完成确认。索引建立仍须明确操作，不自动上传。

| 当前检查 | 结果 |
|---|---|
| 全套共享测试 | **242 项通过，0 失败，2 项既有大库测试默认忽略** |
| `cargo fmt --check`、`git diff --check` | 通过 |
| macOS / Windows `--all-targets` Clippy，`-D warnings` | 两者通过 |
| Windows GNU Release | 构建通过；PE32+ GUI x86-64，仅系统 DLL / CRT 导入 |
| 当前共享 `--core-check` | 内存可查、保存前磁盘无历史、退出后恢复，三项均通过 |
| Windows-only fixtures | **29 项仅交叉 Clippy 编译检查，0 项执行**；Claude 16 项、Embedding 8 项、原生图片 5 项 |

新增回归覆盖查询覆盖与可靠命令、分段状态和保存期限、向量保存游标、增量维护、元数据和原始格式保留，以及后台任务/预览合并、失败恢复、智能策略和状态缓存。共享检查使用真实 SQLite、线程和 channel；Windows-only 源码最后的失焦与退出提示保护已再次通过 Windows Clippy。没有调用真实服务或使用用户密钥。

共享全套命令为 `TMPDIR=/Volumes/QPBuildTemp/tmp CARGO_TARGET_DIR=/Volumes/QPBuildTemp/target CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --locked -- --test-threads=1`。宿主 SSD 空间不足导致过 `SQLITE_FULL` / `SQLITE_IOERR`，最终共享测试和 core-check 使用本次创建的临时 HFS+ RAM 卷；源码、报告及最终 Windows Release 位于项目目录。测试证明功能行为，不代表 SSD 持久性、Windows 运行或真实性能测量。临时卷在交付前清理，不修改用户数据库。

三个任务的独立审查及整轮最终审查均通过，无未解决的 Critical / Important / Minor。审查中发现的状态扫描阻塞保存/退出、失败加载长期占据槽位两项问题已修复并定点复审；测试辅助函数的 panic 清理也已补齐。最终源代码为 `b667a97`，此后仅更新验证文档。

本轮没有重新运行大型性能基准，以下历史毫秒数据不能当作本轮提速比例。常驻内存、空闲 CPU、热键延迟、GUI/IME、多屏 DPI、WinHTTP、DPAPI 和真实服务效果仍需 Windows 实机验收。

## 性能优化时的实施决定

以下按决定顺序保留审查与实施中的取舍，便于追溯：

1. 将“都修复一下”视为批准上一轮具体建议，直接落实计划；若默认交互选择不适合，可在设置调整。
2. 沿用指定文件夹和 `codex/quick-panel` 分支，不创建新工作树；节省磁盘并保留小提交，代价是修改在当前目录可见。
3. 新引擎事件先提供最小 Windows 兼容分支，再接入最终界面；代价是中间提交不展示完整状态，最终版本已接齐。
4. 实施与 Cargo 串行执行以避免共享文件冲突和磁盘峰值；代价是用时较长。
5. 项目没有远端且仅有当前开发分支，交付保留该分支和文件夹；未执行合并、推送或外部发布，如需分发仍须另行操作。
6. 预览保持一个真实 Load 和最新待处理目标，兼容 DIB 在后台准备；代价是必须保留已取消但尚未结束任务的占位，旧任务结束后才能继续最新任务。
7. SSD 持续空间不足时，使用本次新建的 1 GiB RAM 文件卷执行测试；代价是该结果不能证明 SSD 或 Windows 的性能和耐久性。
8. 增加带请求编号的 `LoadFailed`，让失败加载精确释放占位；代价是事件接口增加一个终态，真实存储错误仍需展示，不能用通用 Error 清空所有槽位。
9. 同身份的后台刷新使用独立请求编号而不撤销当前动作；代价是必须严格限定身份相同，真实查询/范围变化或目标消失继续撤销。
10. 增加带请求编号的 `SemanticStatusFailed`，终结失败或退出中的状态扫描；代价是状态请求失败后须通过明确操作或后续变更重试，不做紧密自动重试。

## AI 接口反馈与连接测试（历史验证 e171b3e）

Claude 与 Embedding 接口的连接、认证、协议或网络任务入队失败，在搜索页安静恢复本地检索状态。普通结果、选择和排队中的本地检索请求仍可用；过期回复不能取消新请求。索引接口失败停止本次会话，不自动重试，保留已接收向量。存储和磁盘错误仍通过原有错误路径显示。

两个原生 API 设置表单均提供“测试连接”，使用当前尚未保存的地址、模型、认证和输入密钥；密码留空复用已保存的 DPAPI 密钥，勾选清除密钥禁止发送。未启用功能也可以测试。每次显式点击只发送一次内置文本，不发送历史、路径或文档，不保存配置或写入索引。表单显示成功耗时、失败原因，以及 Embedding 向量维度；编辑配置后提示重新测试。

测试在后台执行，两个表单共享一个实际在途槽。取消或关闭只撤销结果接收，不等待网络；旧请求结束前不叠加第二次测试，旧结果不显示到新表单。测试期间禁止修改和保存，取消保持可用。凭据和请求沿用已有擦除及错误脱敏流程。

| 当前检查 | 结果 |
|---|---|
| 全套共享测试 | **201 项通过，0 失败，2 项既有大库测试默认忽略** |
| `cargo fmt --check`、`git diff --check` | 通过 |
| macOS / Windows `--all-targets` Clippy，`-D warnings` | 同源两者通过 |
| Windows GNU Release | 构建通过；PE32+ GUI x86-64，仅系统 DLL / CRT 导入 |
| 当前共享 `--core-check` | 内存可查、保存前磁盘无历史、退出后恢复，三项均通过 |
| Windows-only fixtures | **24 项仅交叉 Clippy 编译检查，0 项执行**；Claude 16 项、Embedding 8 项 |

新增共享测试 11 项：连接测试 worker / gate / 凭据策略 5 项，查询与索引反馈 6 项。采用真实线程和 channel 验证单在途、取消后保留实际占位、关闭不等待和旧回复隔离；成功检索及索引存储确认路径仍覆盖。便携测试经过红绿验证；6 项新增 Windows fixture 仅交叉编译。独立代码审查未发现需要修复的缺陷。

最终全套命令为 `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --locked -- --test-threads=1`。验证期间宿主空间紧张，曾出现 `SQLITE_FULL` 和 `SQLITE_IOERR_SHMSIZE`；保留故障记录，清理本项目可再生成的构建缓存后，同源码最终全套通过，未修改存储代码来掩盖故障。两项既有大库测试本次未重复。没有调用真实服务或使用用户密钥。

Windows 原生窗口、测试按钮键盘行为、关闭/重开、WinHTTP 和 DPAPI 均未在 macOS 实际执行；需要 Windows 实机验收。此次没有重新测量查询基准，前版测量保留如下。

## 可选 sqlite-vec 语义检索（历史验证 7768dba，2026-10-04）

已实现默认关闭、与 Claude 独立配置的 OpenAI 兼容 Embedding API。地址、模型和密钥独立保存，密钥使用当前 Windows 用户 DPAPI；配置验证不联网。用户明确执行建立/更新索引后，选定范围的保存文本和收藏元数据才会发送给该服务；语义查询仅在点击时发送描述。默认固定内容与收藏，可选全部文本；不解析文件正文、不发送图片或其他二进制表示。

实际向量维度由首批成功结果确定，输入哈希和模型/地址 profile 防止过期内容或不同向量空间混用。已完成向量先在有界内存中参与查询，沿用约 55 秒开始、健康情况下 60 秒窗口的批量事务，缓存预算 4 MiB；压力可提前保存，配置和收藏仍即时保存。正常退出保存原文及所有仍有效的已接收向量，不等待未完成网络；失败保留缓存并允许重试/取消退出。查询、空页和空批次不创建派生表、不提交空事务。普通 Search 不执行新增路径通道或调用模型。

独立审查和定点复审完成。回归覆盖跨提交分页、HashMap 扩容预算、源删除/清理后的批次重验、极值坐标归一化、KNN 限额前的范围过滤、路径优先、菜单目标、收藏/设置取消，以及 Claude 与语义模式恢复查询编号的顺序。最后的顺序回归也经过红绿测试和复审。

| 该版本检查 | 结果 |
|---|---|
| 全套共享测试 | **190 项通过，0 失败，2 项既有大库测试默认忽略** |
| `cargo fmt --check`、`git diff --check` | 通过 |
| macOS / Windows `--all-targets` Clippy，`-D warnings` | 同源两者通过 |
| macOS / Windows GNU Release | 两者构建通过，Windows PE32+ GUI x86-64，只有系统 DLL / CRT 导入 |
| 该版本 Release `--core-check` | 内存可查、保存前磁盘无历史、退出后恢复，三项均通过 |
| Windows-only fixtures | **18 项仅交叉 check / Clippy 编译检查，0 项执行**；包括原 Claude 13 项及 Embedding 5 项 |

最终全套命令为 `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --locked -- --test-threads=1`。最初并行运行在宿主磁盘耗尽时出现两处明确的 `SQLITE_FULL`；清理本项目可再生构建缓存、降低临时数据库并行度后，同源码全套成功，没有以代码修改掩盖环境故障。两项既有 100k/WAL 大库回归此次未重复，历史结果在下文保留。

190 项包括 AI jobs 10、AI 本地检索 9、Claude 协议 15、CLI 1、格式 8、Embedding 协议/输入 15、Embedding jobs 5、热键 4、混合列表 14、持久化 37、搜索 8、语义存储 25、语义会话 17、界面状态 22。没有使用用户密钥或调用付费服务，人工向量不能证明中文语义质量。

同源共享 Release 普通查询基准（真实 COUNT，各 100 次）：

| 实际条数 | 准备耗时 | p50 | p95 | 最大值 |
|---:|---:|---:|---:|---:|
| 10,000 | 2,199 ms | 6.024 ms | 9.053 ms | 9.074 ms |
| 100,000 | 40,746 ms | 7.595 ms | 37.784 ms | 45.305 ms |

同源共享 Release 向量核心基准：两个独立临时数据库，各确认 10,000 条已持久化向量、384 维，每轮 100 次实际 Engine 查询，返回 50 个真实条目。使用人工单位向量、无网络服务；测量不含模型推理、API、Windows 消息和绘制。路径场景在空闲 actor 的临时库批量生成原始收藏/FTS fixture，后续索引接收、vec0 保存和查询均走真实引擎；不是日常收藏保存频率测量。

| 场景 | p50 | p95 | 最大值 | 临时数据库大小 |
|---|---:|---:|---:|---:|
| 10,000 条文本 / 384 维 | 18.105 ms | 18.168 ms | 19.125 ms | 36,978,688 字节 |
| 10,000 条路径元数据 / 384 维 | 19.671 ms | 21.213 ms | 22.754 ms | 32,645,120 字节 |

路径优先通道按 128 条元数据分页并只保留 50 个候选，不读取正文或图片。完整检查避免明确路径被词法限额提前挤掉；耗时仍随路径总数增长。上述 macOS 数值不能作为 Windows 端到端时延或更大路径集合的保证。真实服务兼容、模型效果/费用/响应时间及 Windows GUI、WinHTTP、DPAPI、输入法和系统结束会话仍需实机验收。

实现选择及过程说明按决定顺序保留：沿用用户指定文件夹中的现有开发分支；独立 API 首版采用 OpenAI 兼容格式；只清理向量输入中的不可发送控制字符而保留原文；用 f64 归一化保护 native f32 余弦计算；复用既有表单并修正一处测试写法；为修复收尾引入的确定性 Claude 编号回归，超出执行技能的修复波次数上限做了一次最小定点修复；保留独立语义路径元数据通道，其线性扫描成本见上表。未合并其他分支、推送或发布外部资源。

## 可选 Claude AI 检索（历史版本 b5658ba）

已实现默认关闭的可选“按意思查找”。配置地址、模型与认证密钥后手动调用 Claude Messages 接口，仅发送当前查询、范围和公开功能目录；模型返回短查询、范围/时间和已知功能建议，交由本地引擎检索真实条目。AI 不自动执行功能、不上传历史正文、不建立向量索引，不因检索写入数据库。

设置密钥采用 Windows 当前用户 DPAPI；密文与设置同一事务即时保存。配置本地校验不联网，输入错误通过 owned Message effect 显示，避免后续检索覆盖错误。原有历史窗口、缓存预算、WAL 维护和退出保存流程保持。

最终检查：共享 **125 项通过、0 失败、2 项既有大库检查默认忽略**；macOS 与 Windows all-targets 严格 Clippy（`-D warnings`）、fmt、Windows Release 链接/打包及共享 core-check 均通过。125 项包括 AI jobs 10、AI 本地检索 9、Claude 协议 15、CLI 1、格式 8、热键 4、混合列表 11、持久化 37、搜索 8、界面状态 22。

Windows-only 的 **13 项本地 TCP/DPAPI 测试仅完成交叉编译，没有在此 macOS 上执行**，不计入125项。没有读取用户环境密钥或调用真实收费 API；真实服务协议兼容、响应质量、Windows WinHTTP/DPAPI/GUI/本地时区仍需实机验收。

独立最终审查及修正范围复审通过。新增撤销回归先复现失败，随后验证隐藏/设置取消/取消失败退出不会重新采用已撤销计划的本地回执或派生缓存；普通缓存在没有已采用计划时保持。网络任务惰性启动，一在途加一待处理；取消只停止采用旧结果，在途 HTTP 可能继续完成，阶段超时和读取预算不是独立硬取消 watchdog。AI 线程不持有数据库，退出不等待其非持久化网络请求。

同版共享 Release 查询基准（真实 COUNT，各 100 次查询；不含模型 API、Windows UI）：

| 实际条数 | 准备耗时 | p50 | p95 | 最大值 |
|---:|---:|---:|---:|---:|
| 10,000 | 2,376 ms | 4.564 ms | 9.048 ms | 9.154 ms |
| 100,000 | 37,174 ms | 7.610 ms | 39.330 ms | 40.784 ms |

两项旧大库 WAL/特殊候选回归本次未重新运行，其历史结果保留在下文。新增真实 SQLite 用 data_version 与计数验证同义词、日期筛选、pending、重复检索没有提交；时间条件在所有候选池限额前应用。时间按首次保存，本地日期范围包含今天；不会把使用时间当每次复制的完整日志。

当时的 AGENTS.md/MCP/RAG 与开源检索评估见[设计文档](superpowers/specs/2026-10-03-optional-ai-search-design.md)。sqlite-vec 后续已按上节接入；MCP、完整 RAG 和文件正文解析尚未实现。

## 统一检索与可选快捷键更新（历史版本）

新安装默认只注册呼出面板的全局热键；两组快捷键均能在设置中修改或留空关闭，升级保留原来已保存的组合。注册新组合失败时撤回本次新增注册，保留原绑定，交换两组已拥有的组合不重新抢占。关闭全部热键后仍可点击托盘打开。

功能目录包括设置与快捷键、添加文件夹和文档、暂停或恢复记录、展开或收起预览、切换四种内容范围、保存后退出。它在内存里参与中文、拼音、首字母与关键词匹配，和保存内容合并为最多 50 条明确标注类型的结果。功能不会入库、不会发送为保存内容的 Load / Pin / Copy 操作。空查询优先最近内容；只有内容查询完成且确实为空时才显示添加和设置入口。

最终独立审查及范围复审通过。本次修复了菜单模态期间目标被异步结果替换后误执行其他内容、同一次回车按压自动重复执行新内容，以及空查询加载中短暂显示添加入口三项问题。结果菜单校验打开时的身份与交互状态；回车自动重复被忽略，方向键和输入法仍保留原生行为。

| 本次最终检查 | 结果 |
|---|---|
| `cargo fmt --check` | 通过 |
| `CARGO_INCREMENTAL=0 cargo test --locked` | **89 项通过，0 项失败，2 项原有大库测试默认忽略** |
| macOS / Windows all-targets Clippy，`-D warnings` | 两者均通过 |
| Windows GNU / macOS Release 构建 | 两者均通过 |
| 当前 Release `--core-check` | 内存可查、保存前磁盘无历史、退出后恢复，三项均通过 |
| 新增独立审查与修正复审 | 无未解决的 Critical / Important |

89 项包括 CLI 1 项、格式 8 项、快捷键 4 项、统一检索 9 项、持久化 37 项、搜索 8 项、界面状态 22 项。真实 SQLite 检查了自定义快捷键配置立即保存，以及搜索期间待提交历史保持可见且不提前落盘、功能不进入保存数据。两项大库测试本次未再运行，其基础版的独立结果见下文。

另用当前 Release 库在 macOS aarch64 测量内存功能目录加 50 条保存内容元数据的匹配与合并，1,000 次混合查询：p50 **0.044125 ms**、p95 **0.071083 ms**、最大 **0.366833 ms**。此处包括元数据复制、相关度排序与功能匹配，不包括 SQLite 查询、Windows 消息与窗口绘制，不能解释为端到端响应时间。

以下保留基础版本 `c12fae1` 的核心数据库测量。该部分的原日期也是 2026-10-03；当时的统一检索更新未修改 Engine、Repository、FTS 或原有保存策略。

## 基础版本的检查

| 检查 | 最终结果 |
|---|---|
| `cargo fmt --check` | 通过 |
| `CARGO_INCREMENTAL=0 cargo test --locked` | 72 项通过，0 项失败；2 项大库检查默认忽略，另行运行通过 |
| macOS `cargo clippy --locked --all-targets -- -D warnings` | 通过 |
| Windows `cargo clippy --locked --target x86_64-pc-windows-gnu --all-targets -- -D warnings` | 通过；覆盖 Windows 源码及测试目标编译检查 |
| `bash scripts/build-cross.sh` | Windows Release 构建与 ZIP 打包通过 |
| `quick-panel --core-check` | 保存前内存可查、保存前磁盘无历史、退出后恢复，三项均通过 |
| 独立审查 | 存储、原生集成、界面生命周期及全分支审查完成；提出的 Critical / Important 问题均已解决并范围复审 |
| 分发检查 | PE32+ GUI x86-64；仅导入 Windows 系统 DLL / CRT；ZIP 完整性通过 |

72 项包括 CLI 1 项、剪贴板格式 8 项、持久化 36 项、搜索 8 项、界面状态 19 项。共享测试覆盖真实 SQLite 提交失败、缓存预算和退避、首次变更期限、正常退出恢复、收藏即时保存与删除、中文和拼音搜索、升级索引保留图片，以及迟到粘贴和退出错误状态。

Windows CI 配置已提交，但未触发远程运行。MSVC 构建和 PowerShell 打包脚本尚未在 Windows 执行。

## 基础版本的核心查询性能

基础版本 `c12fae1` 由主代理重新执行 Release 基准，分别写入并用 `COUNT` 确认 10,000 / 100,000 条混合中文、拼音和路径数据；每轮执行 100 次核心查询。

| 实际条数 | 数据准备耗时 | p50 | p95 | 最大值 |
|---:|---:|---:|---:|---:|
| 10,000 | 2,807 ms | 6.038 ms | 9.083 ms | 9.130 ms |
| 100,000 | 47,181 ms | 10.549 ms | 39.309 ms | 46.672 ms |

此处测量共享 Engine 查询往返，宿主为 macOS，不包含 Windows 热键、窗口显示和绘制。满足设计中的核心搜索目标（10k p95 ≤30 ms，100k p95 ≤60 ms）；不应解释为 Windows 端到端延迟。准备阶段主动分批提交来限制输入队列内存，不代表日常保存频率。

同版源码的两个大库 Release 回归由修复代理另外运行通过：

- 100k 特殊候选测试保留旧别名、标题、固定项、收藏和已使用项，10 次查询 p95 为 24.153 ms。
- 100k 连续提交测试在每批保存后采样，WAL 峰值为 66,505,072 字节，发生 13 次回收或复用回落，结束时为 10,934,512 字节。这是批次边界采样，不能作为文件系统瞬时峰值的硬上限。

## 低频写入和退出

新历史进入内存即参与搜索。首次变更建立保存期限，持续复制不推迟它；默认约 55 秒开始事务，为 60 秒保存窗口预留时间。缓存压力可提前保存，故障则保留已接受数据并退避重试。60 秒窗口依赖磁盘正常响应，不是故障 I/O 下的硬实时保证。

收藏、固定项及其删除、确认后的配置立即保存。普通历史删除和使用统计随批次保存；使用统计不重写内容、图片或 FTS 索引。空闲没有变更时，不提交空事务，不周期性重写日志、配置和缩略图。缩略图只保存在内存。WAL 维护由期限和压力触发，并受忙等待、退避和完整 checkpoint 条件限制。

正常退出会关闭新任务入口、排空已接受任务、提交缓存、关闭数据库，之后才结束进程。保存失败时等待用户重试或取消退出，保留缓存；取消后恢复正常后台保存。关闭面板只是隐藏。系统结束会话采用独立的 3 秒有界保存，强制结束或断电仍可能丢失尚未提交的历史。

实现采用的具体边界：固定剪贴板内容视为收藏并即时提交；捕获队列和待保存内容各允许 16 MiB，合计原始载荷峰值可约 32 MiB，单条原始表示与派生文本上限 8 MiB；当前缩略图输出上限 4 MiB、可选兼容 DIB 上限 64 MiB，详见本轮记录。常驻总内存、空闲 CPU、热键显示延迟仍是 Windows 待测指标。

实现期间作出的三项取舍，按决定顺序记录如下：

1. 固定剪贴板视为收藏并即时保存，遵循用户的收藏保存要求；代价是每次明确固定会多一次即时事务。
2. 捕获队列和待保存区分别限制为 16 MiB，便于各自控制压力；代价是合计载荷可达约 32 MiB，若 Windows 总内存超出目标，需要重新协调预算。
3. 在互不重叠的文件范围并行实现，再统一提交和集成检查，以满足当前协作要求；若接口边界判断遗漏，会增加集成返工。最终集成审查和回归已通过。

## 基础版本审查后修正

最终审查修正了候选截断造成的保存后搜索丢失及拼音排序、退出失败的重复弹窗、收藏删除未即时保存、提交期限缺少裕量，以及 Shift+Enter 释放前粘贴。独立复审通过，增加的回归也纳入上述 72 项。FTS 版本为 4，老库升级在事务内完成，不改动原有图片 BLOB；同版本重开不重复建索引。

## Windows 实机验收

在 Windows 10 / 11 x64 解压后运行 `QuickPanel.exe`，重点检查以下尚未实测的原生交互：

1. 托盘、重复启动、全局快捷键、隐藏后召回和退出；任务管理器中的常驻内存及空闲 CPU。
2. 中文输入法选词、键盘导航、长按与快速 Shift+Enter、多屏不同 DPI、浅色和深色主题。
3. 文本、HTML / RTF、图片和文件列表的捕获与还原；目标应用焦点变化、权限差异下的自动或手动粘贴。
4. 拖入收藏、编辑别名和标签、打开及重新定位目标；设置里修改、交换或关闭热键，以及冲突时保留原绑定。
5. 缓存期内正常退出再重开、保存失败后重试或取消，以及 Windows 注销 / 关机。
6. 从搜索执行设置、添加、暂停和预览功能；长按回车、菜单等待期间刷新结果，不应激活其他保存内容；空查询加载不显示临时添加入口。
7. 两个 API 设置页用未保存配置和未启用功能测试；留空/清除密钥、重复点击、测试中关闭再重开及修改配置后的状态。接口失败时本地结果应仍可操作；保存和磁盘错误应继续显示。

8. 连续快速输入、后台新增历史、建立大批索引时维持键盘响应；历史首次变更后持续复制不会推迟保存。只保存向量不应使索引重复从头扫描。
9. 对断开网络盘、失效文件、慢路径执行预览/打开；切换目标、关闭面板或取消后旧结果不能操作新目标，错误后可重新操作。外部再次呼出可直接覆盖旧查询；后台刷新不重置列表滚动和文字选区。
10. 用大尺寸高压缩 PNG、PNG 加超预算 DIB、只有超预算 DIB 分别验证捕获与还原；前两者按预算保留 PNG，只有超预算 DIB 时明确未记录。不同粘贴目标验证 PNG/兼容 DIB；长文本预览能滚动和局部复制，提示只预览部分内容。
11. Auto/Claude/Semantic 三种选择下检查唯一智能按钮、取消、未启用配置引导、空索引引导及手动更新。0 向量不发查询；状态检查中失焦、改查询、切换范围和退出后不发起过期网络请求，退出保存提示不被旧状态失败覆盖。
12. 固定文本、图片和富文本条目编辑名称、别名、标签，再次复制同一内容并重启，确认元数据保留且正文/格式不变；取消或目标失效不提交。核对来源、命中片段和本地时间（含夏令时），以及各设置表单的键盘导航、主题与 DPI。

13. 在“常用推荐设置”启用后，直接从资源管理器访问从未收藏的目录并稳定停留；确认自动项出现、停留/重复聚焦不反复计数、导航后可记录。Win11在多个标签页间切换，确认只记录当前可见文件系统视图；“此电脑”等虚拟位置跳过。
14. 常用推荐的打开、定位、复制路径、收藏和不再推荐；与同请求已有收藏去重，独立最近访问时间正确。设置排除目录应包含子目录而不误伤相似前缀；关闭隐藏自动项，清空保留人工收藏。取消/重新配置后旧探测不能回填。
15. Recent首次启动仅建基线，支持的应用后续打开文档才形成补充记录。用隔离测试夹具验证带参数链接不推荐、通知失败30秒退避重建、基线不回放以及Explorer独立工作；不要把有界补充当作完整文档打开记录。
16. 从搜索、菜单及可选截图热键进入；反向拖动、负坐标、多屏不同DPI、Esc/右键/显示布局变化/失去鼠标捕获，确认区域与取消行为正确。截图不包含已关闭的选区层；PNG色序、方向、透明度及接收应用粘贴符合预期。
17. 编码期间复制新文本，旧截图不得覆盖；暂停记录/排除本工具来源时仍能复制截图但不入历史，正常情况只入库一次。超预算/编码或剪贴板故障应区分反馈；退出不等待编码，已入队截图保存后才退出。确认三组热键的冲突回滚及旧配置升级。

## 可执行文件识别

历史基础版本 `QuickPanel.exe`：4,341,248 字节，SHA-256：

```
6ba87a1d3956b067087b54ad945e8b8e4a1a25570cd345c02b5ff5c5afff624b
```

程序无需 WebView、Electron 或随包携带的 GCC 运行库。ZIP 同时包含使用说明、设计与实施文档、此验证记录、MIT 许可证及依赖版权声明。
