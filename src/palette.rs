//! Functions live only in memory; saved results continue to come from the engine.
use crate::{
    model::{ItemKind, ItemSummary, StoredItem},
    search,
};
use std::cmp::Reverse;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Function {
    Settings,
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
}
impl Default for Context {
    fn default() -> Self {
        Self {
            capture_paused: false,
            preview_open: false,
            content_ready: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FunctionResult {
    pub action: Function,
    pub title: &'static str,
    pub description: &'static str,
    score: i64,
}

#[derive(Clone, Debug)]
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
        Function::PauseCapture => !context.capture_paused,
        Function::ResumeCapture => context.capture_paused,
        Function::ShowPreview => !context.preview_open,
        Function::HidePreview => context.preview_open,
        _ => true,
    }
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
