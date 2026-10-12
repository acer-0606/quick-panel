//! Shared, side-effect-free settings navigation and scoped edits consumed by Win32.
use crate::{
    form_validation::FormError,
    model::{Settings, SmartSearchMode, Theme},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    General,
    Data,
    Paths,
    Smart,
    Organizer,
    Services,
    Accounts,
}
impl Group {
    pub const ALL: [Self; 7] = [
        Self::General,
        Self::Data,
        Self::Paths,
        Self::Smart,
        Self::Organizer,
        Self::Services,
        Self::Accounts,
    ];
    pub fn title(self) -> &'static str {
        match self {
            Self::General => "常规与快捷键",
            Self::Data => "剪贴板与数据",
            Self::Paths => "常用路径",
            Self::Smart => "智能查找",
            Self::Organizer => "AI 管家",
            Self::Services => "服务连接",
            Self::Accounts => "账号填充",
        }
    }
    pub fn actions(self) -> &'static [Action] {
        match self {
            Self::General => &[Action::General],
            Self::Data => &[Action::Data],
            Self::Paths => &[Action::Paths],
            Self::Smart => &[Action::Smart],
            Self::Organizer => &[
                Action::Organizer,
                Action::Advanced,
                Action::Review,
                Action::Memory,
            ],
            Self::Services => &[Action::Claude, Action::Embedding],
            Self::Accounts => &[Action::Accounts, Action::AccountManage, Action::AccountLock],
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Accounts,
    AccountManage,
    AccountLock,
    General,
    Data,
    Paths,
    Smart,
    Organizer,
    Advanced,
    Review,
    Memory,
    Claude,
    Embedding,
}
impl Action {
    pub fn title(self) -> &'static str {
        match self {
            Self::Accounts => "编辑启用、提示与闲置锁定",
            Self::AccountManage => "管理账号保险箱",
            Self::AccountLock => "立即锁定账号保险箱",
            Self::General => "编辑主题与三个快捷键",
            Self::Data => "编辑记录、排除与保留",
            Self::Paths => "编辑观察与排除",
            Self::Smart => "编辑能力、首选与索引范围",
            Self::Organizer => "编辑开关、模式与范围",
            Self::Advanced => "高级调度参数",
            Self::Review => "管理已有整理建议",
            Self::Memory => "管理已有整理记忆",
            Self::Claude => "配置共用 Claude 连接",
            Self::Embedding => "配置独立 Embedding 连接",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    Main,
    Tray,
    Search,
}
pub fn entry_group(_: Entry) -> Group {
    Group::General
}
pub fn function_group(f: crate::palette::Function) -> Option<Group> {
    use crate::palette::Function;
    Some(match f {
        Function::Settings => entry_group(Entry::Search),
        Function::ScreenshotSettings => Group::General,
        Function::PathActivitySettings => Group::Paths,
        Function::AiSettings | Function::EmbeddingSettings => Group::Services,
        Function::OrganizerSettings => Group::Organizer,
        _ => return None,
    })
}
pub const RETENTION_NOTE: &str =
    "减少保留限额会按策略清理未固定历史；固定内容和收藏始终保留。仅浏览设置不会清理数据。";
fn positive(raw: &str, max: usize, field: usize) -> Result<usize, FormError> {
    raw.trim()
        .parse::<usize>()
        .ok()
        .filter(|n| (1..=max).contains(n))
        .ok_or_else(|| FormError::new(field, format!("请输入1至{max}的整数")))
}
fn count(v: &[String], n: usize) -> Result<(), FormError> {
    if v.len() == n {
        Ok(())
    } else {
        Err(FormError::new(0, "表单字段不完整"))
    }
}
/// Merge only the editor's fields into the current settings, never a stale other-group snapshot.
pub fn edit(current: &Settings, action: Action, v: &[String]) -> Result<Settings, FormError> {
    let mut next = current.clone();
    match action {
        Action::Accounts => {
            count(v, 3)?;
            next.account_fill.enabled = v[0] == "1";
            next.account_fill.show_hint = v[1] == "1";
            next.account_fill.lock_minutes = positive(&v[2], 60, 2)? as u32;
        }
        Action::General => {
            count(v, 4)?;
            next.theme = match v[0].as_str() {
                "0" => Theme::System,
                "1" => Theme::Light,
                "2" => Theme::Dark,
                _ => return Err(FormError::new(0, "请选择主题")),
            };
            let mut keys = Vec::new();
            for (i, raw) in v.iter().enumerate().skip(1) {
                let key = crate::hotkeys::parse_hotkey(raw).map_err(|e| FormError::new(i, e))?;
                if key != (0, 0) && keys.contains(&key) {
                    return Err(FormError::new(i, "已启用的快捷键须互不相同"));
                }
                keys.push(key);
            }
            (next.hotkey_modifiers, next.hotkey_key) = keys[0];
            (next.clipboard_hotkey_modifiers, next.clipboard_hotkey_key) = keys[1];
            (next.screenshot_hotkey_modifiers, next.screenshot_hotkey_key) = keys[2];
        }
        Action::Data => {
            count(v, 4)?;
            next.capture_paused = v[0] == "1";
            next.excluded_apps = v[1]
                .split([',', '，'])
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect();
            // Unusual persisted values are displayable; unchanged values are not normalized.
            if v[2] != current.retention_days.to_string() {
                next.retention_days = positive(&v[2], 36500, 2)? as u32;
            }
            if v[3] != current.max_history.to_string() {
                next.max_history = positive(&v[3], 1000000, 3)?;
            }
        }
        Action::Smart => {
            count(v, 4)?;
            next.ai.enabled = v[0] == "1";
            next.embedding.enabled = v[1] == "1";
            next.smart_search = match v[2].as_str() {
                "0" => SmartSearchMode::Auto,
                "1" => SmartSearchMode::Claude,
                "2" => SmartSearchMode::Semantic,
                _ => return Err(FormError::new(2, "请选择首选能力")),
            };
            next.embedding.index_scope = match v[3].as_str() {
                "0" => crate::semantic::IndexScope::PinnedAndFavorites,
                "1" => crate::semantic::IndexScope::AllText,
                _ => return Err(FormError::new(3, "请选择索引范围")),
            };
            if next.ai.enabled
                && let Some(error) = crate::organizer_ui::organizer_config_warning(&next.ai)
            {
                return Err(FormError::new(0, error));
            }
            if next.embedding.enabled {
                crate::embedding::parse_endpoint(&next.embedding.endpoint)
                    .map_err(|e| FormError::new(1, e))?;
                crate::embedding::validate_model(&next.embedding.model)
                    .map_err(|e| FormError::new(1, e))?;
                if next.embedding.encrypted_key.is_empty() {
                    return Err(FormError::new(1, "请先在服务连接配置 Embedding 密钥"));
                }
            }
        }
        _ => return Err(FormError::new(0, "此编辑使用专用表单")),
    }
    Ok(next)
}
pub fn description(group: Group, s: &Settings, index: &str, query: Option<&str>) -> String {
    let enabled = |v| if v { "已启用" } else { "已停用" };
    match group {
        Group::Accounts => format!("账号填充{}；前台提示{}。\r\n闲置{}分钟锁定。\r\n\r\n关闭后仍可管理保险箱，但不能填入。主密码无法找回，请保管好主密码和数据库密文备份。提示仅用于已解锁且核验通过的前台密码框；点击后才填入，不提交登录。",enabled(s.account_fill.enabled),enabled(s.account_fill.show_hint),s.account_fill.lock_minutes),
        Group::General => {
            let key=|m,k| {let value=crate::hotkeys::format_hotkey(m,k);if value.is_empty(){"已关闭".into()}else{value}};
            format!("主题：{}\r\n面板：{}\r\n剪贴板：{}\r\n区域截图：{}\r\n\r\n三个快捷键集中配置。留空关闭对应快捷键；已启用的组合须互不相同。",match s.theme {Theme::System=>"跟随系统",Theme::Light=>"浅色",Theme::Dark=>"深色"},key(s.hotkey_modifiers,s.hotkey_key),key(s.clipboard_hotkey_modifiers,s.clipboard_hotkey_key),key(s.screenshot_hotkey_modifiers,s.screenshot_hotkey_key))
        },
        Group::Data => format!("记录{}\r\n未固定历史：{}天未使用 / 最多{}条\r\n排除应用：{}\r\n\r\n{}",if s.capture_paused {"已暂停；已有历史与截图复制仍可使用"} else {"已开启"},s.retention_days,s.max_history,if s.excluded_apps.is_empty(){"无".into()}else{s.excluded_apps.join("、")},RETENTION_NOTE),
        Group::Paths => format!("常用路径{}。可观察资源管理器与最近文档，管理排除目录。\r\n清空访问统计及不再推荐名单后，后续新访问可重新推荐。",enabled(s.path_activity.enabled)),
        Group::Smart => format!("Claude 按意思查找{}；语义查找{}。\r\n首选：{}；索引范围：{}\r\n{}\r\n\r\n索引状态：{}\r\n{}\r\n\r\n服务地址、密钥、模型和测试请到“服务连接”。建立/更新索引可从面板索引入口或功能搜索执行。",enabled(s.ai.enabled),enabled(s.embedding.enabled),match s.smart_search {SmartSearchMode::Auto=>"自动",SmartSearchMode::Claude=>"Claude",SmartSearchMode::Semantic=>"语义"},match s.embedding.index_scope {crate::semantic::IndexScope::PinnedAndFavorites=>"固定内容与收藏",crate::semantic::IndexScope::AllText=>"全部文本"},if !s.ai.enabled && !s.embedding.enabled {"请启用所需能力；配置不足时先到服务连接。"}else{"首选能力决定智能查找的选择；关闭检索不影响独立 AI 管家。"},index,query.unwrap_or("本次没有智能查询状态")),
        Group::Organizer => format!("后台整理{}；整理记忆{}。\r\n模式：{}\r\n范围：{}\r\n核心设置包含开关、模式、范围、整理说明和记忆。间隔、批量、模型轮次放在高级参数。\r\n\r\n{}\r\n{}\r\n管理页加载后显示实际建议与记忆；此处不推测未加载数量。",enabled(s.organizer.enabled),enabled(s.organizer.memory_enabled),match s.organizer.mode {crate::organizer::OrganizerMode::Suggest=>"生成建议，由我确认",crate::organizer::OrganizerMode::Automatic=>"仅填空别名，追加标签"},match s.organizer.scope {crate::organizer::OrganizerScope::SavedAndPaths=>"收藏、固定剪贴板和外部路径",crate::organizer::OrganizerScope::SavedOnly=>"收藏和固定剪贴板",crate::organizer::OrganizerScope::PathsOnly=>"外部常用路径",crate::organizer::OrganizerScope::AllTextAndPaths=>"全部文本与外部路径"},if s.organizer.enabled {"可管理已有建议和记忆。"}else{"管家已停用：后台不再整理，仍可管理已有建议与记忆。"},if s.organizer.memory_enabled {"记忆用于规则与反馈；具体生效条件见管理页。"}else{"记忆已停用：停止使用已有规则和记录新反馈。"}),
        Group::Services => "Claude 兼容 API：检索与 AI 管家共用地址、密钥、模型与推理强度。\r\nEmbedding：独立服务连接与模型。\r\n\r\n能力开关、首选和索引范围在“智能查找”；管家开关在“AI 管家”。\r\n配置完整不代表连接成功；进入编辑后可手动获取模型或测试，结果仅对应当时的配置。浏览本页不会发起网络请求。".into(),
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub top_navigation: bool,
    pub navigation: Rect,
    pub detail: Rect,
    pub action: Rect,
    pub open: Rect,
    pub close: Rect,
}
/// Physical client size and monitor DPI; the native detail is an independently scrollable Edit.
pub fn layout(width: i32, height: i32, dpi: u32) -> Layout {
    let s = |v: i32| v * dpi.max(96) as i32 / 96;
    let pad = s(12).min(width / 12);
    let gap = s(8);
    let row = s(32);
    let footer = height - pad - row;
    let top = width < s(650);
    let navw = s(172).min(width / 3);
    let navigation = if top {
        Rect {
            x: pad,
            y: pad,
            width: width - 2 * pad,
            height: row,
        }
    } else {
        Rect {
            x: pad,
            y: pad,
            width: navw,
            height: (footer - pad - gap).max(row),
        }
    };
    let x = if top { pad } else { pad + navw + gap };
    let y = if top { pad + row + gap } else { pad };
    let w = (width - x - pad).max(1);
    let action_y = footer - row - gap;
    let detail = Rect {
        x,
        y,
        width: w,
        height: (action_y - y - gap).max(1),
    };
    Layout {
        top_navigation: top,
        navigation,
        detail,
        action: Rect {
            x,
            y: action_y,
            width: w,
            height: row,
        },
        open: Rect {
            x: pad,
            y: footer,
            width: (width - 3 * pad) / 2,
            height: row,
        },
        close: Rect {
            x: 2 * pad + (width - 3 * pad) / 2,
            y: footer,
            width: (width - 3 * pad) / 2,
            height: row,
        },
    }
}

/// Entire path submission command policy, also consumed by the native leaf.
pub struct PathSubmission {
    pub next: crate::path_activity::PathActivitySettings,
    pub clear: bool,
    pub commands: Vec<crate::model::Command>,
}
pub fn submit_paths(current: &Settings, values: &[String]) -> Result<PathSubmission, FormError> {
    count(values, 4)?;
    let next = crate::path_activity::PathActivitySettings {
        enabled: values[0] == "1",
        recent_documents: values[1] == "1",
        excluded_paths: crate::path_observer::parse_exclusions(&values[2])
            .map_err(|e| FormError::new(2, e))?,
    };
    let clear = values[3] == "1";
    let mut commands = Vec::new();
    if next != current.path_activity {
        let mut settings = current.clone();
        settings.path_activity = next.clone();
        commands.push(crate::model::Command::SaveSettings(settings));
    }
    if clear {
        commands.push(crate::model::Command::ClearPathActivity);
    }
    Ok(PathSubmission {
        next,
        clear,
        commands,
    })
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Accounts,
    Hub(Group),
    Memory,
    Review,
}
/// Explicit native Review return preserves the Memory frame and feedback source.
#[derive(Clone, Debug, Default)]
pub enum ReviewExit {
    #[default]
    Closed,
    Memory(Option<(String, crate::harness_tools::FeedbackTarget)>),
}
/// A navigation frame carries its selected group and the entire source chain.
#[derive(Default)]
pub struct Navigation {
    frames: Vec<Page>,
}
impl Navigation {
    pub fn review_exit(
        &mut self,
        exit: &ReviewExit,
        memory: &mut crate::organizer_ui::MemoryUiState,
    ) -> bool {
        if matches!(exit, ReviewExit::Memory(_)) {
            self.open_with_memory(Page::Memory, memory);
            true
        } else {
            false
        }
    }
    pub fn open_with_memory(
        &mut self,
        page: Page,
        memory: &mut crate::organizer_ui::MemoryUiState,
    ) {
        if self.open(page) {
            memory.enter_navigation_frame();
        }
    }
    pub fn close_with_memory(
        &mut self,
        memory: &mut crate::organizer_ui::MemoryUiState,
    ) -> Option<Page> {
        if self.current().is_some() {
            memory.leave_navigation_frame();
        }
        self.close()
    }
    pub fn clear_with_memory(&mut self, memory: &mut crate::organizer_ui::MemoryUiState) {
        while self.current().is_some() {
            self.close_with_memory(memory);
        }
    }
    pub fn clear(&mut self) {
        self.frames.clear();
    }
    pub fn allows_panel_query(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn current(&self) -> Option<Page> {
        self.frames.last().copied()
    }
    pub fn open(&mut self, page: Page) -> bool {
        if self.current() == Some(page) {
            return false;
        }
        self.frames.push(page);
        true
    }
    pub fn select_group(&mut self, group: Group) {
        if let Some(Page::Hub(selected)) = self.frames.last_mut() {
            *selected = group;
        }
    }
    pub fn close(&mut self) -> Option<Page> {
        self.frames.pop();
        self.current()
    }
    pub fn back_label(&self) -> &'static str {
        match self.frames.iter().rev().nth(1) {
            Some(Page::Accounts) => "返回账号",
            Some(Page::Memory) => "返回记忆",
            Some(Page::Review) => "返回建议",
            Some(Page::Hub(_)) => "返回设置",
            None => "关闭设置",
        }
    }
}

#[derive(Clone, Debug)]
pub enum EditorFieldKind {
    Hidden,
    Line,
    Multiline,
    Choice,
    Switch(String),
}
#[derive(Clone, Debug)]
pub struct EditorField {
    pub label: String,
    pub kind: EditorFieldKind,
}
#[derive(Clone, Debug)]
pub struct EditorSpec {
    pub fields: Vec<EditorField>,
    pub note: Option<String>,
    pub test_label: Option<String>,
    pub models: bool,
}
#[derive(Clone, Debug)]
pub struct EditorFieldLayout {
    pub label: Rect,
    pub control: Rect,
    pub hidden: bool,
    pub dropdown: bool,
}
#[derive(Clone, Debug)]
pub struct EditorLayout {
    pub fields: Vec<EditorFieldLayout>,
    pub note: Option<Rect>,
    pub test_button: Option<Rect>,
    pub model_button: Option<Rect>,
    pub test_result: Option<Rect>,
    pub content_height: i32,
}
/// Conservative native Segoe UI 15px wrapping, in physical client coordinates.
pub fn wrapped_height(text: &str, width: i32, dpi: u32) -> i32 {
    let s = |v: i32| v * dpi.max(96) as i32 / 96;
    ((text.chars().count() as i32 * s(15) + width.max(1) - 1) / width.max(1)).max(1) * s(22)
}
impl EditorSpec {
    pub fn layout(&self, width: i32, dpi: u32) -> EditorLayout {
        let s = |v: i32| v * dpi.max(96) as i32 / 96;
        let x = s(18);
        let w = (width - s(48)).max(1);
        let mut y = s(12);
        let mut fields = Vec::new();
        for field in &self.fields {
            let hidden = matches!(field.kind, EditorFieldKind::Hidden);
            let label_height = wrapped_height(&field.label, w, dpi);
            let control_height = match &field.kind {
                EditorFieldKind::Multiline => s(120),
                EditorFieldKind::Switch(text) => {
                    wrapped_height(text, (w - s(24)).max(1), dpi).max(s(28))
                }
                _ => s(28),
            };
            fields.push(EditorFieldLayout {
                label: Rect {
                    x,
                    y,
                    width: w,
                    height: label_height,
                },
                control: Rect {
                    x,
                    y: y + label_height + s(4),
                    width: w,
                    height: control_height,
                },
                hidden,
                dropdown: matches!(field.kind, EditorFieldKind::Choice),
            });
            if !hidden {
                y += label_height + control_height + s(12);
            }
        }
        let note = self.note.as_ref().map(|text| {
            let r = Rect {
                x,
                y,
                width: w,
                height: wrapped_height(text, w, dpi),
            };
            y += r.height + s(12);
            r
        });
        let mut test_button = None;
        let mut model_button = None;
        let mut test_result = None;
        if let Some(label) = &self.test_label {
            let bw = if self.models { (w - s(8)) / 2 } else { w };
            let bh = wrapped_height(label, bw, dpi).max(s(32));
            test_button = Some(Rect {
                x,
                y,
                width: bw,
                height: bh,
            });
            if self.models {
                model_button = Some(Rect {
                    x: x + bw + s(8),
                    y,
                    width: bw,
                    height: bh,
                });
            }
            y += bh + s(8);
            test_result = Some(Rect {
                x,
                y,
                width: w,
                height: s(128),
            });
            y += s(140);
        }
        EditorLayout {
            fields,
            note,
            test_button,
            model_button,
            test_result,
            content_height: y + s(12),
        }
    }
}

impl EditorLayout {
    /// Focus indices are fields, then test button, model button, and result reader.
    /// Preserve scaled scroll, clamp to new extent, and reveal any focused content.
    pub fn scroll_after_reflow(
        &self,
        scroll: i32,
        old_dpi: u32,
        dpi: u32,
        viewport_height: i32,
        focus: Option<usize>,
    ) -> i32 {
        let mut result = crate::organizer_ui::clamp_scroll(
            scroll.saturating_mul(dpi as i32) / old_dpi.max(1) as i32,
            self.content_height,
            viewport_height,
        );
        let focused_rect = focus.and_then(|i| {
            if let Some(field) = self.fields.get(i) {
                (!field.hidden).then_some(field.control)
            } else {
                [self.test_button, self.model_button, self.test_result]
                    .get(i - self.fields.len())
                    .copied()
                    .flatten()
            }
        });
        if let Some(r) = focused_rect {
            if r.y < result {
                result = r.y;
            } else if r.y + r.height > result + viewport_height {
                result = (r.y + r.height - viewport_height).max(0);
            }
        }
        crate::organizer_ui::clamp_scroll(result, self.content_height, viewport_height)
    }
}

/// Fixed native error reader stays outside the scrolling input viewport.
pub fn editor_error_rect(width: i32, height: i32, dpi: u32) -> Rect {
    let scale = |n: i32| n * dpi.max(96) as i32 / 96;
    Rect {
        x: scale(18),
        y: height - scale(102),
        width: (width - scale(36)).max(1),
        height: scale(50),
    }
}
