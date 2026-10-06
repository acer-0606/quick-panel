//! Functions live only in memory; saved results continue to come from the engine.
use crate::{
    ai::{AiPlan, FunctionDescriptor},
    model::{ItemKind, ItemSummary, StoredItem},
    search,
};
use std::cmp::Reverse;

#[derive(Clone)]
pub struct RowDetail {
    pub organizer_stage: Option<crate::organizer_display::Stage>,
    pub title: String,
    pub text: String,
    pub saved_at_ms: Option<i64>,
    pub visited_at_ms: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Function {
    Screenshot,
    ScreenshotSettings,
    Settings,
    PathActivitySettings,
    AiSettings,
    OrganizerSettings,
    OrganizerReview,
    AiSearch,
    SmartSearch,
    EmbeddingSettings,
    SemanticSearch,
    UpdateSemanticIndex,
    CancelSemanticIndex,
    AddFolder,
    AddDocument,
    PauseCapture,
    ResumeCapture,
    ShowPreview,
    HidePreview,
    All,
    Clipboard,
    Folders,
    Documents,
    Exit,
}
impl Function {
    fn id(self) -> &'static str {
        match self {
            Self::Screenshot => "function:screenshot",
            Self::ScreenshotSettings => "function:screenshot-settings",
            Self::Settings => "function:settings",
            Self::PathActivitySettings => "function:path-activity-settings",
            Self::AiSettings => "function:ai-settings",
            Self::OrganizerSettings => "function:organizer-settings",
            Self::OrganizerReview => "function:organizer-review",
            Self::SmartSearch => "function:smart-search",
            Self::AiSearch => "function:ai-search",
            Self::EmbeddingSettings => "function:embedding-settings",
            Self::SemanticSearch => "function:semantic-search",
            Self::UpdateSemanticIndex => "function:update-semantic-index",
            Self::CancelSemanticIndex => "function:cancel-semantic-index",
            Self::AddFolder => "function:add-folder",
            Self::AddDocument => "function:add-document",
            Self::PauseCapture => "function:pause",
            Self::ResumeCapture => "function:resume",
            Self::ShowPreview => "function:show-preview",
            Self::HidePreview => "function:hide-preview",
            Self::All => "function:all",
            Self::Clipboard => "function:clipboard",
            Self::Folders => "function:folders",
            Self::Documents => "function:documents",
            Self::Exit => "function:exit",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Context {
    pub capture_paused: bool,
    pub preview_open: bool,
    pub content_ready: bool,
    pub ai_enabled: bool,
    pub embedding_enabled: bool,
    pub indexing: bool,
}
impl Default for Context {
    fn default() -> Self {
        Self {
            capture_paused: false,
            preview_open: false,
            content_ready: true,
            ai_enabled: false,
            embedding_enabled: false,
            indexing: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionResult {
    pub action: Function,
    pub title: &'static str,
    pub description: &'static str,
    score: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SuggestedEntry {
    id: String,
    path: crate::path_activity::PathSuggestion,
}
impl From<crate::path_activity::PathSuggestion> for SuggestedEntry {
    fn from(path: crate::path_activity::PathSuggestion) -> Self {
        use sha2::{Digest, Sha256};
        let id = format!("path-suggestion:{:x}", Sha256::digest(path.key.as_bytes()));
        Self { id, path }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Function(FunctionResult),
    Saved(ItemSummary),
    Suggested(SuggestedEntry),
}
impl Entry {
    pub fn detail(&self, query: &str) -> RowDetail {
        let mut text = self.subtitle();
        if let Some(item) = self.saved()
            && item.kind == ItemKind::Clipboard
            && !query.trim().is_empty()
            && let Some((source, excerpt)) = item.subtitle.split_once(" · ")
        {
            let normalized = excerpt.to_lowercase();
            if let Some(position) = normalized.find(&query.trim().to_lowercase()) {
                let preceding = normalized[..position].chars().count();
                let mut normalized_chars = 0;
                let matched = excerpt
                    .chars()
                    .position(|c| {
                        let at = normalized_chars;
                        normalized_chars += c.to_lowercase().count();
                        at >= preceding
                    })
                    .unwrap_or(0);
                let start = matched.saturating_sub(4);
                let short_source: String = source.chars().take(12).collect();
                text = format!(
                    "{}{} · {}{}",
                    short_source,
                    if source.chars().count() > 12 {
                        "…"
                    } else {
                        ""
                    },
                    if start > 0 { "…" } else { "" },
                    excerpt.chars().skip(start).take(64).collect::<String>()
                );
            }
        }
        RowDetail {
            organizer_stage: None,
            title: self.title().into(),
            text,
            saved_at_ms: self.saved().map(|item| item.created_at_ms),
            visited_at_ms: self.suggested().map(|path| path.last_seen_ms),
        }
    }
    pub fn id(&self) -> &str {
        match self {
            Self::Function(result) => result.action.id(),
            Self::Saved(item) => &item.id,
            Self::Suggested(path) => &path.id,
        }
    }
    pub fn title(&self) -> &str {
        match self {
            Self::Function(result) => result.title,
            Self::Saved(item)
                if item.kind != ItemKind::Clipboard && !item.alias.trim().is_empty() =>
            {
                item.alias.trim()
            }
            Self::Saved(item) => &item.title,
            Self::Suggested(path) if !path.path.alias.trim().is_empty() => path.path.alias.trim(),
            Self::Suggested(path) => &path.path.title,
        }
    }
    pub fn subtitle(&self) -> String {
        match self {
            Self::Function(result) => result.description.into(),
            Self::Saved(item) if !item.alias.is_empty() => {
                format!(
                    "{} · {}{}",
                    item.subtitle,
                    item.alias.chars().take(24).collect::<String>(),
                    if item.alias.chars().count() > 24 {
                        "…"
                    } else {
                        ""
                    }
                )
            }
            Self::Saved(item) => item.subtitle.clone(),
            Self::Suggested(path) => format!(
                "外部观察 · {} 次有效访问 · {}",
                path.path.visits, path.path.path
            ),
        }
    }
    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::Function(_) => "⚙ 功能",
            Self::Suggested(path) => {
                if path.path.kind == ItemKind::Folder {
                    "▣ 常用文件夹"
                } else {
                    "▧ 常用文档"
                }
            }
            Self::Saved(item) => match item.kind {
                ItemKind::Clipboard => "▤ 剪贴板",
                ItemKind::Folder => "▣ 文件夹",
                ItemKind::Document => "▧ 文档",
            },
        }
    }
    pub fn pinned(&self) -> bool {
        self.saved().is_some_and(|item| item.pinned)
    }
    pub fn saved(&self) -> Option<&ItemSummary> {
        if let Self::Saved(item) = self {
            Some(item)
        } else {
            None
        }
    }
    pub fn suggested(&self) -> Option<&crate::path_activity::PathSuggestion> {
        if let Self::Suggested(path) = self {
            Some(&path.path)
        } else {
            None
        }
    }
    /// Action-only adapter. Never load, pin, delete or persist this item as an ID.
    pub fn suggestion_action(&self) -> Option<StoredItem> {
        let path = self.suggested()?;
        Some(StoredItem {
            summary: ItemSummary {
                id: self.id().into(),
                kind: path.kind,
                title: path.title.clone(),
                subtitle: path.path.clone(),
                alias: path.alias.clone(),
                tags: path.tags.clone(),
                pinned: false,
                created_at_ms: 0,
                last_used_ms: path.last_seen_ms,
                use_count: path.visits,
            },
            text: String::new(),
            representations: vec![],
            target: Some(path.path.clone()),
            open_with: None,
        })
    }
    pub fn function(&self) -> Option<Function> {
        if let Self::Function(result) = self {
            Some(result.action)
        } else {
            None
        }
    }
}

/// Keep already-visible rows in place when an older in-flight query completes
/// after a metadata-only AI update. New matches keep their order at the end.
pub fn preserve_visible_order(previous: &[Entry], mut next: Vec<Entry>) -> Vec<Entry> {
    let positions: std::collections::HashMap<_, _> = previous
        .iter()
        .enumerate()
        .map(|(i, row)| (row.id(), i))
        .collect();
    next.sort_by_key(|row| positions.get(row.id()).copied().unwrap_or(usize::MAX));
    next
}

pub fn row_details_with_progress(
    rows: &[Entry],
    query: &str,
    progress: &crate::organizer_display::Progress,
) -> Vec<RowDetail> {
    use crate::organizer::Source;
    let mut details = row_details(rows, query);
    for (row, detail) in rows.iter().zip(&mut details) {
        detail.organizer_stage = progress.items.iter().find_map(|(source, stage)| {
            let matched = match (source, row) {
                (Source::SavedItem { id }, Entry::Saved(item)) => *id == item.id,
                (Source::ObservedPath { normalized_path }, Entry::Suggested(path)) => {
                    *normalized_path == path.path.key
                }
                _ => false,
            };
            matched.then_some(*stage)
        });
    }
    details
}
pub fn apply_organizer_metadata(
    rows: &mut [Entry],
    updates: &[crate::organizer_display::MetadataUpdate],
) -> bool {
    let mut changed = false;
    for row in rows {
        for update in updates {
            changed |= match row {
                Entry::Saved(item) => update.apply_saved(item),
                Entry::Suggested(path) => update.apply_path(&mut path.path),
                Entry::Function(_) => false,
            };
        }
    }
    changed
}

/// Display metadata is built once for a result snapshot, never persisted.
pub fn row_details(rows: &[Entry], query: &str) -> Vec<RowDetail> {
    let paths: Vec<_> = rows
        .iter()
        .map(|entry| {
            entry
                .saved()
                .filter(|item| item.kind != ItemKind::Clipboard)
                .map(|item| item.subtitle.as_str())
                .or_else(|| entry.suggested().map(|path| path.path.as_str()))
        })
        .collect();
    let labels = crate::path_labels::labels(&paths, query);
    rows.iter()
        .enumerate()
        .map(|(index, entry)| {
            let mut detail = entry.detail(query);
            if let Some(Some(label)) = labels.get(index) {
                let custom = entry.saved().is_some_and(|item| {
                    !item.alias.trim().is_empty()
                        || item.title.to_lowercase() != label.leaf.to_lowercase()
                }) || entry
                    .suggested()
                    .is_some_and(|path| !path.alias.trim().is_empty());
                if !custom {
                    detail.title.clone_from(&label.title);
                }
                detail.text = label.context.clone();
                if let Some(path) = entry.suggested() {
                    detail
                        .text
                        .push_str(&format!(" · 外部观察 · {} 次", path.visits));
                }
            }
            detail
        })
        .collect()
}

struct Spec {
    action: Function,
    title: &'static str,
    description: &'static str,
    alias: &'static str,
    keywords: &'static [&'static str],
}
const CATALOG: &[Spec] = &[
    Spec {
        action: Function::Screenshot,
        title: "区域截图",
        description: "拖动选区，将 PNG 图片复制到剪贴板",
        alias: "截图",
        keywords: &["截屏", "屏幕截图", "screenshot", "screen capture"],
    },
    Spec {
        action: Function::ScreenshotSettings,
        title: "截图快捷键设置",
        description: "配置可选截图快捷键；默认关闭，留空可关闭",
        alias: "截图设置",
        keywords: &["截图快捷键", "截图热键"],
    },
    Spec {
        action: Function::PathActivitySettings,
        title: "常用推荐设置",
        description: "可选观察外部资源管理器和 Recent，管理排除目录与统计",
        alias: "常用路径设置",
        keywords: &[
            "常用文件夹设置",
            "自动推荐",
            "路径记录",
            "资源管理器",
            "Recent",
        ],
    },
    Spec {
        action: Function::SmartSearch,
        title: "智能查找",
        description: "按首选能力理解查询；有索引时可查相似内容",
        alias: "智能检索",
        keywords: &["智能搜索"],
    },
    Spec {
        action: Function::EmbeddingSettings,
        title: "语义检索设置",
        description: "配置独立 Embedding API、密钥和索引范围",
        alias: "向量设置",
        keywords: &["Embedding配置", "语义配置"],
    },
    Spec {
        action: Function::SemanticSearch,
        title: "语义查找（已建索引）",
        description: "发送本次查询，检索已建立的语义索引",
        alias: "语义搜索",
        keywords: &["向量检索"],
    },
    Spec {
        action: Function::UpdateSemanticIndex,
        title: "建立/更新语义索引",
        description: "发送选定范围的保存文本与元数据，逐批建立索引",
        alias: "更新语义索引",
        keywords: &["建立索引", "向量索引"],
    },
    Spec {
        action: Function::CancelSemanticIndex,
        title: "取消索引",
        description: "停止后续批次，保留已接收向量",
        alias: "取消语义索引",
        keywords: &["停止索引"],
    },
    Spec {
        action: Function::AiSettings,
        title: "AI检索设置",
        description: "配置可选 Claude 格式 API、模型与密钥",
        alias: "AI设置",
        keywords: &["Claude配置", "AI配置", "API配置", "模型配置"],
    },
    Spec {
        action: Function::OrganizerSettings,
        title: "AI 管家设置",
        description: "设置后台整理的范围、模式、间隔与批次",
        alias: "整理设置",
        keywords: &["AI管家", "自动分类", "自动起名"],
    },
    Spec {
        action: Function::OrganizerReview,
        title: "整理建议",
        description: "查看本地已完成建议，选择应用、编辑或忽略",
        alias: "审阅建议",
        keywords: &["待确认", "分类建议", "别名建议"],
    },
    Spec {
        action: Function::AiSearch,
        title: "Claude 按意思查找",
        description: "使用已配置的 AI 理解检索描述",
        alias: "AI检索",
        keywords: &["AI搜索", "按意思查找"],
    },
    Spec {
        action: Function::Settings,
        title: "设置",
        description: "修改快捷键、外观和记录设置",
        alias: "快捷键",
        keywords: &["打开设置", "热键", "外观", "主题", "settings", "hotkey"],
    },
    Spec {
        action: Function::AddFolder,
        title: "添加文件夹",
        description: "选择常用目录，保存为收藏",
        alias: "收藏文件夹",
        keywords: &["添加", "添加收藏", "添加目录", "folder", "add folder"],
    },
    Spec {
        action: Function::AddDocument,
        title: "添加文档",
        description: "选择文件，保存为快捷文档",
        alias: "收藏文档",
        keywords: &["添加", "添加收藏", "添加文件", "document", "add document"],
    },
    Spec {
        action: Function::PauseCapture,
        title: "暂停剪贴板记录",
        description: "暂停捕获新的剪贴板内容",
        alias: "暂停记录",
        keywords: &["停止记录", "pause", "暂停捕获"],
    },
    Spec {
        action: Function::ResumeCapture,
        title: "恢复剪贴板记录",
        description: "继续捕获新的剪贴板内容",
        alias: "恢复记录",
        keywords: &["继续记录", "resume", "开启记录"],
    },
    Spec {
        action: Function::ShowPreview,
        title: "打开预览",
        description: "展开预览区，查看选中的保存内容",
        alias: "展开预览",
        keywords: &["preview", "预览内容"],
    },
    Spec {
        action: Function::HidePreview,
        title: "收起预览",
        description: "关闭预览区，释放图片预览",
        alias: "关闭预览",
        keywords: &["preview", "隐藏预览"],
    },
    Spec {
        action: Function::All,
        title: "查看全部",
        description: "显示全部保存内容；功能始终可以搜索",
        alias: "",
        keywords: &["全部", "所有", "all"],
    },
    Spec {
        action: Function::Clipboard,
        title: "查看剪贴板",
        description: "切换到剪贴板历史",
        alias: "",
        keywords: &["历史", "粘贴历史", "clipboard"],
    },
    Spec {
        action: Function::Folders,
        title: "查看文件夹",
        description: "切换到常用文件夹",
        alias: "",
        keywords: &["目录", "常用目录", "folders"],
    },
    Spec {
        action: Function::Documents,
        title: "查看文档",
        description: "切换到快捷文档",
        alias: "",
        keywords: &["文件", "快捷文档", "documents"],
    },
    Spec {
        action: Function::Exit,
        title: "保存后退出",
        description: "保存全部缓存后结束程序",
        alias: "退出",
        keywords: &["exit", "quit", "关闭程序"],
    },
];

fn available(action: Function, context: Context) -> bool {
    match action {
        Function::AiSearch => context.ai_enabled,
        Function::SemanticSearch | Function::UpdateSemanticIndex => context.embedding_enabled,
        Function::CancelSemanticIndex => context.embedding_enabled && context.indexing,
        Function::PauseCapture => !context.capture_paused,
        Function::ResumeCapture => context.capture_paused,
        Function::ShowPreview => !context.preview_open,
        Function::HidePreview => context.preview_open,
        _ => true,
    }
}

pub fn ai_catalog(context: Context) -> Vec<FunctionDescriptor> {
    CATALOG
        .iter()
        .filter(|spec| {
            !matches!(spec.action, Function::AiSearch | Function::SmartSearch)
                && available(spec.action, context)
        })
        .map(|spec| FunctionDescriptor {
            id: spec.action.id().into(),
            title: spec.title.into(),
            description: spec.description.into(),
        })
        .collect()
}

pub fn ai_results(plan: &AiPlan, saved: Vec<ItemSummary>, context: Context) -> Vec<Entry> {
    let mut rows = Vec::new();
    for id in &plan.functions {
        if let Some(spec) = CATALOG.iter().find(|spec| {
            !matches!(spec.action, Function::AiSearch | Function::SmartSearch)
                && spec.action.id() == id
                && available(spec.action, context)
        }) && !rows.iter().any(|row: &Entry| row.id() == id)
        {
            rows.push(Entry::Function(FunctionResult {
                action: spec.action,
                title: spec.title,
                description: spec.description,
                score: 1_020_000,
            }));
        }
    }
    rows.extend(saved.into_iter().map(Entry::Saved));
    rows.truncate(50);
    rows
}

fn searchable(spec: &Spec) -> StoredItem {
    // This is an ephemeral search adapter only. Entry::Function remains a distinct
    // type and can never be submitted to the repository or loaded as a saved item.
    StoredItem {
        summary: ItemSummary {
            id: spec.action.id().into(),
            kind: ItemKind::Document,
            title: spec.title.into(),
            subtitle: spec.description.into(),
            alias: spec.alias.into(),
            tags: spec.keywords.iter().map(|word| (*word).into()).collect(),
            pinned: false,
            created_at_ms: 0,
            last_used_ms: 0,
            use_count: 0,
        },
        text: String::new(),
        representations: Vec::new(),
        target: None,
        open_with: None,
    }
}

pub fn results(query: &str, saved: Vec<ItemSummary>, context: Context) -> Vec<Entry> {
    if query.trim().is_empty() {
        if !saved.is_empty() {
            return saved.into_iter().take(50).map(Entry::Saved).collect();
        }
        if !context.content_ready {
            return Vec::new();
        }
        return [
            Function::AddFolder,
            Function::AddDocument,
            Function::Settings,
        ]
        .into_iter()
        .map(|action| {
            let spec = CATALOG
                .iter()
                .find(|spec| spec.action == action)
                .expect("starter is in catalog");
            Entry::Function(FunctionResult {
                action,
                title: spec.title,
                description: spec.description,
                score: 0,
            })
        })
        .collect();
    }
    let mut entries: Vec<_> = saved.into_iter().map(Entry::Saved).collect();
    for spec in CATALOG
        .iter()
        .filter(|spec| available(spec.action, context))
    {
        let item = searchable(spec);
        if search::matches(&item, query) {
            let relevance = search::rank(&item.summary, query);
            entries.push(Entry::Function(FunctionResult {
                action: spec.action,
                title: spec.title,
                description: spec.description,
                // Explicit function names/aliases remain reachable even when
                // fifty pinned saved items share that exact name or alias.
                score: if relevance >= 900_000 {
                    1_020_000
                } else {
                    relevance + 1_000
                },
            }));
        }
    }
    // Stable sorting retains the engine's recency ordering among equally ranked
    // saved items, while exact function names take priority over incidental hits.
    entries.sort_by_cached_key(|entry| {
        Reverse(match entry {
            Entry::Function(result) => result.score,
            Entry::Saved(item) => search::rank(item, query),
            Entry::Suggested(_) => 0,
        })
    });
    entries.truncate(50);
    entries
}

/// Keep actor hybrid/cosine ordering intact; function hits stay searchable.
pub fn semantic_results(query: &str, saved: Vec<ItemSummary>, context: Context) -> Vec<Entry> {
    let functions = results(
        query,
        vec![],
        Context {
            content_ready: false,
            ..context
        },
    );
    let mut entries: Vec<_> = functions
        .into_iter()
        .filter(|entry| matches!(entry, Entry::Function(_)))
        .collect();
    entries.extend(saved.into_iter().map(Entry::Saved));
    entries.truncate(50);
    entries
}

pub fn with_suggestions(
    query: &str,
    mut rows: Vec<Entry>,
    paths: Vec<crate::path_activity::PathSuggestion>,
) -> Vec<Entry> {
    let mut keys: std::collections::BTreeSet<_> = rows
        .iter()
        .filter_map(Entry::saved)
        .filter(|item| item.kind != ItemKind::Clipboard)
        .filter_map(|item| crate::path_activity::normalize_path(&item.subtitle).ok())
        .collect();
    let limit = if query.trim().is_empty() { 5 } else { 10 };
    let mut added = 0;
    for path in paths {
        if rows.len() >= 50 || added >= limit {
            break;
        }
        if let Ok(key) = crate::path_activity::normalize_path(&path.path)
            && keys.insert(key)
        {
            rows.push(Entry::Suggested(path.into()));
            added += 1;
        }
    }
    rows.truncate(50);
    rows
}
