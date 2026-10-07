# Windows 账号填充 Implementation Plan

> For agentic workers: REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** 实现用户已批准的普通Windows权限账号填充、简单教学与场景推荐第一版。

**Architecture:** 四个任务串行：共享目标/场景策略，加密保险箱与仅密文actor存储，Windows UIA/点选/填入后台实现，原生账号界面与设置/提示整合。密码始终隔离于普通内容和AI，所有异步动作绑定窗口与解锁代次。

**Tech Stack:** Rust2024、Win32/windows0.62.2、现有SQLite actor、RustCrypto Argon2id/XChaCha20Poly1305/zeroize。既有Pi/Node不变。

**Spec:** docs/superpowers/specs/2026-10-07-account-fill-design.md

## Global Constraints

- 中文交互；Windows 10/11 x64，Rust/Win32原生、现有theme、IME、DPI、滚动与固定footer。不引入WebView、Electron、新的常驻Node进程或全局快捷键。
- 密码只进入独立加密保险箱/临时受控填入路径，不进入普通剪贴板、Item/搜索/RAG/AI/Pi/遥测/异常文本。不得读取、导入或探测用户真实密码库；测试只用合成数据。
- 主线程不运行KDF、解密、UIA跨进程查询或磁盘I/O。KDF/加密与UIA各有一个有界实际工作槽，取消/锁定/退出不join阻塞线程；过期结果拒绝，实际占用未结束不伪装空闲、不累积替代线程。
- 只允许用户明确点击的填入。每次填入绑定保险箱代次、目标窗口实例、来源应用、所见场景和字段；关闭/失焦到第三方/换连接/锁定/退出撤销许可，迟到结果不可操作新目标。
- 保存错误保留当前编辑内容；只有关联本次请求的持久化回执可报告已保存。点击保存后已接纳的操作包括后台加密准备和actor密文写入；锁定/关闭撤销填充和显示但不默默取消该保存。正常退出异步等待此流水线落盘后再发送Engine Shutdown，失败提供重试或取消退出；未点击保存的草稿可取消清理。
- 普通历史仍保持默认60秒缓冲/正常退出保存；账号与关联的人工修改立即持久化。浏览、匹配、提示、填充不提交事务、不发网络请求、不扫描普通正文或图片BLOB。
- Cargo全程串行：CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0。Rust子进程测试固定QP_TEST_NODE=/Users/chuwujie/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin/node。
- 工作目录/分支为/Users/chuwujie/workspace/quick-panel与codex/quick-panel；不创建worktree、不push、不修改全局环境。root只维护docs/scratch，单一实现者；不回退他人修改。
- 本机macOS共享测试和Windows交叉编译不等于Windows实机；GUI/UIA/IME/多屏/锁屏实际未运行必须明示。产品中不显示测试实现细节。

---

### Task 1: 共享场景、教学与填充许可

**Files:** 新增src/account_fill.rs、tests/account_fill.rs；修改src/lib.rs、src/model.rs中Settings默认字段。此任务不改native、存储或AI代码。

**Interfaces:** 导出AccountFillSettings（enabled=false、show_hint=true、lock_minutes=5）；AppIdentity（规范路径/文件指纹）、WindowIdentity（handle/pid/process_start）、FieldSelector（有界持久控件条件）、LiveField（无密码值）、SceneSnapshot（应用/窗口/非秘密场景/字段）、TargetCondition、Binding（account_id引用）、MatchDecision（Exact/ApplicationOnly/Rejected）、FillTicket/FillGate；target_matches(expected: &str, observed: Option<&str>) -> bool由生产matcher调用。最终精确类型与函数签名写报告，后续任务以报告为接口权威。

语义合同：match_binding(binding, snapshot)只对同应用、唯一有效字段及已存在且一致的目标给Exact；target=None只能ApplicationOnly。密码字段必须is_password/visible/enabled/writable；不允许username/password同字段。持久选择器不得含RuntimeId、固定坐标或密码内容。FillGate绑定WindowIdentity与vault_epoch，明确开始/撤销/验证；真正下发前仍必须重采样场景。

- [ ] 写真实策略RED，从旧宽松候选判断接线后验证跨应用/跨服务器/缺失目标/多个字段/已锁定拒绝，不用缺函数编译失败代替行为RED。测试最小行为：

    use quick_panel::account_fill::target_matches;
    #[test]
    fn different_or_missing_target_never_matches() {
        assert!(!target_matches("db-prod.example:5432", Some("db-test.example:5432")));
        assert!(!target_matches("db-prod.example:5432", None));
        assert!(target_matches("db-prod.example:5432", Some("db-prod.example:5432")));
    }

完整matcher测试必须进一步构造真实Binding/SceneSnapshot，不仅测试字符串helper；覆盖ApplicationOnly不得升格Exact、相同字段重复、同HWND不同PID/start、不同HWND同应用、撤销/代次过期、teach取消不产生绑定、上限拒绝。

- [ ] 实现共享类型、校验、场景构造/匹配及填充票据，全部函数将由Task3/4实际消费，不另建测试平行逻辑。单次非秘密snapshot最多256节点/8层，必须有完整采集标记；超时/截断的部分结果不能证明唯一或保存教学/Exact/填入；selector串与anchors设明确上限；目标值做保守首尾空白规范化，不做子串或模糊目标匹配。
- [ ] 新Settings字段必须serde(default)向后兼容，锁定分钟1–60、默认5；关闭模式策略可测试，不新增I/O。
- [ ] 定点cargo test --locked --test account_fill；随后host/Windows all-target strictClippy/check、fmt/diff。此阶段不full/Release/Node。
- [ ] 自审后只提交owned文件，写task-1-report.md：精确所有公开类型/签名、约束、真实RED/GREEN命令输出、native后续接线要求与Windows未执行边界。

### Task 2: 加密保险箱、后台密码操作与仅密文存储

**Files:** 新增src/account_vault.rs、src/account_vault_store.rs、tests/account_vault.rs；按职责可新增src/account_vault_jobs.rs。修改src/lib.rs、src/model.rs（仅Vault命令/事件）、src/engine.rs、src/repository.rs（窄接线）、src/platform/windows/ui.rs（仅新增事件的穷尽match兼容臂，Task4接真实UI消费；此时无产品UI发送这些命令）、Cargo.toml/Cargo.lock、THIRD_PARTY_NOTICES.txt及必要许可生成材料。

**Interfaces:** 消费Task1 Binding与AccountFillSettings；导出Secret（Debug脱敏、Drop zeroize）、AccountSummary、AccountDraft、VaultDocument、VaultSession及后台单槽VaultWorker；具体返回/状态以实现报告签名交给Task4。存储使用以下精确类型合同，命名可局部等价但报告必须明确：

    pub struct VaultSnapshot { pub revision: u64, pub envelope: Vec<u8> }
    Command::ReadAccountVault { request_id: u64 }
    Command::WriteAccountVault { request_id: u64, expected_revision: u64, envelope: Vec<u8> }
    Event::AccountVaultLoaded { request_id: u64, result: Result<Option<VaultSnapshot>, String> }
    Event::AccountVaultWritten { request_id: u64, result: Result<u64, String> }

actor仅处理密文，不能拥有主密码/明文Account/Scene；写入前校验封套格式/版本/长度，拒绝普通文本或无效头部。表懒创建，singleton id=1，CAS和相同密文幂等；请求body算队列预算，单包4MiB。独立回执不触发普通Changed/semantic_revision。未创建时只读返回None且不新建表。

- [ ] 写合成crypto及真实SQLite actor RED：从可编译但未提供创建/解锁能力的实现获得行为RED，不引入可运行的明文存储过渡方案；实现后检验明文名称/用户名/密码/绑定目标在DB/WAL中均不存在。测试正确/错误主密码、每次nonce不同、tag/格式/参数篡改、截断、过大文档、随机失败处理；不以仅检查密文!=明文代替实际解密/完整性断言。
- [ ] 使用RustCrypto库固定Argon2id v19参数64MiB/3/1与32-byte密钥、XChaCha20Poly1305随机salt16/nonce24，OS随机源与zeroize；核验实际crate版本/MSRV/Windows支持并锁定Cargo.lock，不自行实现算法。格式上限/参数先验证再KDF；Secret Debug/错误不输出原文。设置/解锁/修改主密码12–256字符/1024bytes，账号/关联上限遵循spec。
- [ ] VaultSession只存解锁密钥及有界文档；公开摘要无密码；password留空编辑保留旧值、新增不可空；删除账号同时删除其关联。每次锁定递增epoch并清除摘要/秘密；主密码不长期保存。后台KDF/序列化单实际工作槽，取消后不join也不伪装任务完成，过期结果清理。
- [ ] 添加独立密文SQLite表/actor消息；真实SQL触发器模拟写失败，确认旧密文可解锁、原请求可重试、CAS冲突不覆盖、重试已提交同密文无额外事务。读/匹配/锁定不增加写入；Shutdown排空已发送密文命令；提供单一已接纳保存状态，锁定/关闭不取消保存加密准备，迟到回执不重新解锁。Task4在发送Engine Shutdown前等待已接纳保存settle；未点击保存的草稿和解锁/查询任务可撤销。加入准备阶段锁定/关闭、原密文重试、迟到成功不解锁的回归。不得把vault作为ContentSource暴露给Pi/普通搜索。
- [ ] 针对Task2测试：cargo test --locked --test account_vault --test account_fill，并对受影响的真实persistence/engine/harness隔离用例做定点；再host/Windows all-target strictClippy/check、fmt/diff。不full/Release，不触碰真实API或用户数据库。新Rust依赖许可必须更新。
- [ ] 提交owned文件，task-2-report.md给出VaultSession/Worker/Secret/命令事件的精确签名、保存准备→回执接纳的用法、错误保留/锁定/退出合同与验证日志。

### Task 3: Windows 目标捕获、UIA后台、点选教学和定向填入

**Files:** 新增src/platform/windows/account_target.rs、account_worker.rs、account_picker.rs、tests/windows_account_fill.rs；可按单一职责合并/拆分此三文件。修改src/platform/windows/mod.rs和Cargo.toml的Windows features，必要时Task1共享策略及其测试做真实接线补充。此任务不做App主界面整合。

**Interfaces:** 消费Task1 SceneSnapshot/Binding/FillGate与Task2 Secret。提供快速非UIA的capture_foreground(own_pid)->Result<WindowIdentity,..>；后台单槽AccountWorker执行inspect/point-teach/validate-and-fill，输出只含非秘密DTO/完成状态；提供原生pick(owner,target,role)点选入口。所有精确函数签名、消息enum与取消方法写报告交Task4。

- [ ] 用生产helper写RED：目标绑定在面板显示前；两个应用/同进程两窗口/进程创建时间变化/目标控件消失拒绝；填入需要vault_epoch及当前ticket、窗口/场景复核；多个/不可写/隐藏/非密码字段拒绝，密码不经过剪贴板或按键序列。
- [ ] 面板弹出后的GetFocusedElement不得作为原目标；使用已捕获的窗口范围/快速GUI线程焦点信息，无法保留具体字段时在原窗口内唯一定位或点选。UIA在独立MTA线程创建使用和释放，COM对象不跨到GUI线程；设置连接/事务超时，所有查询有256节点/8层/2秒预算，逐步TreeWalker/有界缓存遍历，不以全子树FindAll后截断冒充有界；未完整采集则返回失败或请求点选小范围重采样。工作槽忙时不再建线程，取消只撤销结果和填充许可，不join主线程。事件/观察回调只投递轻量通知，不在callback做UIA/文件指纹。文件指纹首次/变化时后台读取并缓存，读取失败拒绝匹配。
- [ ] inspect仅提取控件结构和非输入标签；不读取密码控件Name/Value，普通输入值默认不采集，仅用户点选的非密码目标可读取。ElementPath不持久化RuntimeId/绝对像素。自动唯一定位密码框与可选用户名；有歧义返回TeachingRequired。
- [ ] 点选只记录元素/目标条件，不录制键盘、不截屏。覆盖Esc/右键取消、原目标切换/退出、同窗口字段重建和多屏DPI物理坐标。已知目标的选择限制在该窗口，未知目标让用户明确选一个输入框再捕获其窗口。
- [ ] 填入路径拿到用户点击票据后逐步重新核验目标/焦点/epoch/scene并定向SetValue；密码BSTR及可控UTF16缓冲用完清理。没有可写ValuePattern即返回中文“不支持此输入框”，不做隐式Clipboard/SendInput/Tab/Enter或自动登录。跨进程操作不能提供原子事务，已发出的写入不能宣称可撤回；停止后续写入并如实报告部分完成。
- [ ] Windows-only合成原生窗口fixtures覆盖实际UIA字段读取/写入/双窗口/取消，macOS只交叉编译记录0执行；portable facade覆盖命令门禁不是模拟另一套策略。串行host相关tests与Windows all-target strictClippy/check/fmt/diff，不full/Release。提交并写task-3-report.md，说明所有native调用顺序/handle所有权与Task4接线。

### Task 4: 原生账号流程、设置、提示与最终交付

**Files:** 新增src/platform/windows/account_ui.rs、account_hint.rs，按需新增src/account_fill_ui.rs及tests/account_fill_ui.rs；修改src/platform/windows/ui.rs、dialogs.rs、settings_ui.rs、src/settings_ui.rs、palette.rs、model.rs所需UI字段和对应tests；更新README.md/docs/verification.md。不得在UI里实现第二份密码加密或UIA逻辑。

**Interfaces:** 消费前三任务报告公开API，所有操作使用实际匹配/VaultWorker/AccountWorker。新增账号设置分组及Fill/Manage/Lock可搜索功能；主面板只提供本地命令，账号操作不加入AI/Pi可执行工具或AI函数目录；保险箱内容只在专属原生账号窗口内查找。保持现有设置导航及跨页草稿合同。

- [ ] 先为真实UI状态策略写RED：disabled/locked/uninitialized/busy/failure分开；保存错误保持raw；旧Vault回执不能覆盖新session；面板自身前台不覆盖目标；切第三方/锁定/退出取消票据；取消窗口不等待worker。学习未能确定目标必须显示“仅按应用推荐”，浏览器场景无可信站点时不保存自动关联。
- [ ] 原生窗口完成创建/解锁/管理/新增编辑删除/主密码更改/关联移除、原生安全密码Edit与IME/可滚长错误/DPI固定footer。账号搜索仅解锁后本地RAM执行；锁定时清显示。首次用户流程和“填充并记住/仅填这次/重新指定位置/换账号”全部接线，无法识别时仍能明确选目标，而不是要求写规则。
- [ ] App.show_panel在展示前快速捕获目标票据，后台inspect可稍后完成但只能更新本票据。账户面板标题持续显示“将填入：应用/目标”；对被调用弹窗/面板focus迁移与真正切至第三方作区分。关闭、显示布局变化、原目标消失、vault锁定、配置禁用等同步撤销提示和任务。
- [ ] 设置新增“账号填充”：启用、前台提示、闲置1–60分钟、管理、锁定。默认关闭；禁用后无hook/浮层，管理仍可达但不能填入。只对已解锁且已验证Exact关联的前台密码框显示不激活提示；点击才授权填入。使用WinEvent/UIA轻量焦点事件唤醒有界后台检查，不周期性全桌面扫描。会话锁定/休眠通知立即撤销并锁定。
- [ ] Vault编辑准备新密文并发actor，关联本请求回执后才接纳；失败保留草稿且Retry原sealed请求。先保存关联成功再执行仍有效的填入；目标期间改变只保存关联而不迟到填入。fill确认不自动提交登录；不支持控件清楚报告。退出先异步完成已点击保存的加密准备/密文写入，成功后才让Engine Shutdown排空；失败给重试或取消退出。锁定/关闭不取消已接纳保存，不接受其迟到结果恢复解锁/填入；未点击保存的草稿可关闭清理。
- [ ] 对新增真实消费者运行定点RED/GREEN及跨模块回归。冻结实现后一次cargo test --locked -- --test-threads=1（固定Node）；host/Windows all-target strictClippy/check、fmt/diff；Windows Release与共享core-check严格串行。Windows原生fixtures执行0不能算通过运行。
- [ ] 更新README实际流程/权限/支持范围/主密码无法找回与密文备份，更新新增Rust依赖许可；不宣称第三方软件全部兼容或Windows实测。原脚本freshpack使用本计划node-assets内复制并核验的既有runtime（不下载、不改Node），逐成员source/stage/ZIP/CRC/SHA/manifest/canonical校验；记录实际大小、hash及源码版本。命令metadata与产物JSON使用不同名。
- [ ] 自审所有A1–A8，提交owned实现和文档，task-4-report.md含精确接口、全部命令/失败/平台边界/最终产物。root随后逐任务gate及一次全变更审查，最后归档并清理仅本计划scratch。
