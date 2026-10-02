# Quick Panel

Windows 原生快捷面板，用一个入口找回剪贴板内容、打开常用文件夹和文档。Rust、Win32 原生控件与 SQLite；新历史先在内存里可用，再低频批量保存。

## 在 Windows 使用

将 dist/QuickPanel-Windows-x64.zip 复制到 Windows x64 电脑，解压后双击 QuickPanel.exe。程序常驻系统托盘，可通过托盘图标或快捷键打开面板。此包面向 Windows 10 和 Windows 11。

| 操作 | 默认方式 |
|---|---|
| 打开面板 | Ctrl+Alt+Space |
| 直接打开剪贴板 | Ctrl+Alt+V |
| 移动选择 | 上下方向键 |
| 打开文件或粘贴历史 | Enter |
| 纯文本粘贴历史 | Shift+Enter |
| 复制文字或结果 | Ctrl+C，搜索框有文本选区时优先复制文字 |
| 更多动作 | Ctrl+K 或右键 |
| 切换全部、剪贴板、文件夹、文档 | Ctrl+1 至 Ctrl+4 |
| 添加收藏 | 添加按钮，或拖入文件和文件夹 |
| 关闭面板 | Esc、关闭按钮或点击面板外 |
| 真正退出 | 托盘菜单的退出程序 |

快捷键可在设置中修改，保留系统 Win+V。主题可选择跟随系统、浅色或深色；设置中可以暂停记录及排除指定应用。收藏支持名称、别名、标签、打开方式和目标重新定位。图片与文本预览按需打开，隐藏面板会释放图片预览。

中文输入框使用原生控件；输入法选词时，回车、Esc 和方向键先交给输入法。已展示结果保持当前选择，不因后台新增历史跳到另一项。

## 数据和保存

数据位于 %LOCALAPPDATA%/QuickPanel/quick-panel.sqlite3 及 SQLite 的同目录 WAL 文件。内容只在本机保存，当前没有云同步或磁盘加密。来源应用声明的敏感剪贴板标记会被排除，也可以手动排除应用；这依赖来源应用正确提供标记。

- 历史进入内存后立即可查、可复制和粘贴。保存期限从最早待保存项开始计算，正常磁盘响应下最多缓存 60 秒，连续复制不会延期。
- 收藏与确认后的配置立即保存。使用次数合并更新，随批次保存。
- 没有数据变更时不提交空事务，不周期性重写配置、日志或缩略图。
- 图片原始表示与数据库引用在同一个事务内保存。缩略图仅作内存缓存。
- 待保存内容及捕获队列各有 16 MiB 预算，单条捕获总表示和派生文字上限 8 MiB；接近预算时提前保存。超限时说明未记录，不截断当前剪贴板。
- 正常退出先排空已接受任务并保存全部待保存内容，收到成功确认后才结束进程。失败可重试或取消退出，缓存继续保留。
- 任务管理器强制结束、系统故障或断电可能丢失尚未提交的数据。Windows 结束会话执行有界保存，不能等同正常退出。

备份时先正常退出程序，再复制整个 QuickPanel 数据目录。

## 构建和验证

在 Windows 安装 Rust 和 Visual Studio C++ 构建工具后：

    cargo test --locked
    cargo clippy --locked --all-targets -- -D warnings
    powershell -ExecutionPolicy Bypass -File scripts/build-windows.ps1

在安装 Rust GNU Windows 目标及 MinGW 的 macOS/Linux 上：

    rustup target add x86_64-pc-windows-gnu
    bash scripts/build-cross.sh

共享核心可在非 Windows 环境运行：

    cargo run --release -- --core-check
    cargo run --release -- --benchmark 10000
    cargo run --release -- --benchmark 100000

core-check 使用临时数据库验证保存前可检索、保存前磁盘没有历史条目，以及退出后能够恢复。benchmark 写入明确数量的混合中文、拼音、路径记录，输出 100 次核心查询的 p50、p95 和最大值；不包含 Windows 窗口与渲染延迟。基准准备阶段主动分批保存，以限制输入队列内存，不代表日常自动保存频率。

自动测试覆盖低频提交、真实 SQLite 故障重试、退避与缓存预算、退出恢复、中文短查询和拼音检索、格式解析与面板状态。Windows CI 配置位于 .github/workflows/windows.yml；本地交叉构建成功不等于该 CI 已运行。

实际 Windows 输入法、多屏 DPI、托盘、图片绘制、不同权限窗口粘贴和系统关机需要实机验收。程序使用普通权限；自动粘贴遇到权限或焦点限制时保留已复制内容并说明手动粘贴。

## 设计与计划

- [设计文档](docs/superpowers/specs/2026-10-02-windows-quick-panel-design.md)
- [实施计划](docs/superpowers/plans/2026-10-02-windows-quick-panel.md)

MIT 许可证见 LICENSE。依赖库的许可与版权声明见 THIRD_PARTY_NOTICES.txt。
