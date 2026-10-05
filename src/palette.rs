//! Functions live only in memory; saved results continue to come from the engine.
use crate::{
    ai::{AiPlan, FunctionDescriptor},
    model::{ItemKind, ItemSummary, StoredItem},
    search,
};
use std::cmp::Reverse;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Function {
    Settings,
    AiSettings,
    AiSearch,
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
            Self::Settings => "function:settings",
            Self::AiSettings => "function:ai-settings",
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
pub enum Entry {
    Function(FunctionResult),
    Saved(ItemSummary),
}
impl Entry {
    pub fn id(&self) -> &str {
        match self {
            Self::Function(result) => result.action.id(),
            Self::Saved(item) => &item.id,
        }
    }
    pub fn title(&self) -> &str {
        match self {
            Self::Function(result) => result.title,
            Self::Saved(item) => &item.title,
        }
    }
    pub fn subtitle(&self) -> String {
        match self {
            Self::Function(result) => result.description.into(),
            Self::Saved(item) if !item.alias.is_empty() => {
                format!("{} · {}", item.alias, item.subtitle)
            }
            Self::Saved(item) => item.subtitle.clone(),
        }
    }
    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::Function(_) => "⚙ 功能",
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
    pub fn function(&self) -> Option<Function> {
        if let Self::Function(result) = self {
            Some(result.action)
        } else {
            None
        }
    }
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
        action: Function::EmbeddingSettings,
        title: "语义检索设置",
        description: "配置独立 Embedding API、密钥和索引范围",
        alias: "向量设置",
        keywords: &["Embedding配置", "语义配置"],
    },
    Spec {
        action: Function::SemanticSearch,
        title: "语义查找",
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
        action: Function::AiSearch,
        title: "按意思查找",
        description: "使用已配置的 AI 理解检索描述",
        alias: "AI检索",
        keywords: &["AI搜索", "智能查找"],
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
        title: "暂停记录",
        description: "暂停捕获新的剪贴板内容",
        alias: "暂停剪贴板",
        keywords: &["停止记录", "pause", "暂停捕获"],
    },
    Spec {
        action: Function::ResumeCapture,
        title: "恢复记录",
        description: "继续捕获新的剪贴板内容",
        alias: "恢复剪贴板",
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
        .filter(|spec| spec.action != Function::AiSearch && available(spec.action, context))
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
            spec.action != Function::AiSearch
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
