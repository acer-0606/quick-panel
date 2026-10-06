use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemKind {
    Clipboard,
    Folder,
    Document,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope {
    #[default]
    All,
    Clipboard,
    Folder,
    Document,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Representation {
    /// CF_UNICODETEXT, CF_DIB, CF_DIBV5, CF_HDROP, PNG, HTML Format, Rich Text Format.
    pub format: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct ClipboardSnapshot {
    pub text: String,
    pub representations: Vec<Representation>,
    pub source_app: String,
    pub captured_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemSummary {
    pub id: String,
    pub kind: ItemKind,
    pub title: String,
    pub subtitle: String,
    pub alias: String,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub created_at_ms: i64,
    pub last_used_ms: i64,
    pub use_count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredItem {
    pub summary: ItemSummary,
    pub text: String,
    pub representations: Vec<Representation>,
    pub target: Option<String>,
    pub open_with: Option<String>,
}

#[derive(Clone, Debug)]
pub struct FavoriteInput {
    pub id: Option<String>,
    pub kind: ItemKind,
    pub title: String,
    pub target: String,
    pub alias: String,
    pub tags: Vec<String>,
    pub open_with: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SmartSearchMode {
    #[default]
    Auto,
    Claude,
    Semantic,
}
#[derive(Clone, Debug)]
pub struct ClipboardMetadataInput {
    pub id: String,
    pub title: String,
    pub alias: String,
    pub tags: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticIndexStatus {
    pub profile: String,
    pub indexed: usize,
    pub needs_update: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub path_activity: crate::path_activity::PathActivitySettings,
    pub ai: crate::ai::AiSettings,
    pub organizer: crate::organizer::OrganizerSettings,
    pub embedding: crate::semantic::EmbeddingSettings,
    pub theme: Theme,
    pub smart_search: SmartSearchMode,
    pub hotkey_modifiers: u32,
    pub hotkey_key: u32,
    pub clipboard_hotkey_modifiers: u32,
    pub clipboard_hotkey_key: u32,
    pub screenshot_hotkey_modifiers: u32,
    pub screenshot_hotkey_key: u32,
    pub capture_paused: bool,
    pub excluded_apps: Vec<String>,
    pub retention_days: u32,
    pub max_history: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            path_activity: crate::path_activity::PathActivitySettings::default(),
            ai: crate::ai::AiSettings::default(),
            organizer: crate::organizer::OrganizerSettings::default(),
            embedding: crate::semantic::EmbeddingSettings::default(),
            theme: Theme::System,
            smart_search: SmartSearchMode::Auto,
            hotkey_modifiers: 3,
            hotkey_key: 32,
            clipboard_hotkey_modifiers: 0,
            clipboard_hotkey_key: 0,
            screenshot_hotkey_modifiers: 0,
            screenshot_hotkey_key: 0,
            capture_paused: false,
            excluded_apps: Vec::new(),
            retention_days: 30,
            max_history: 10_000,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SearchQuery {
    pub text: String,
    pub scope: Scope,
    pub request_id: u64,
}

#[derive(Clone, Debug, Default)]
pub struct AiSearchQuery {
    pub queries: Vec<String>,
    pub scope: Scope,
    pub from_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub request_id: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SaveStats {
    pub semantic_status_changed: bool,
    pub pending_items: usize,
    pub pending_bytes: usize,
    pub commits: u64,
    pub last_saved_at_ms: i64,
}

#[derive(Clone, Debug)]
pub enum Command {
    OrganizerDetail {
        request_id: u64,
        source: crate::organizer::Source,
        expected_version: String,
    },
    OrganizerReview {
        request_id: u64,
        decision: crate::organizer_ui::ReviewDecision,
    },
    OrganizerTick,
    OrganizerComplete {
        token: u64,
        result: Result<Vec<crate::organizer::Proposal>, String>,
    },
    OrganizerSnapshot {
        request_id: u64,
    },
    OrganizerProgress {
        request_id: u64,
    },
    OrganizerApply {
        proposals: Vec<crate::organizer::Proposal>,
    },
    OrganizerDismiss {
        sources: Vec<crate::organizer::Source>,
    },
    OrganizerRetry,
    ObservePaths(Vec<crate::path_activity::PathObservation>),
    SuppressPath(String),
    ClearPathActivity,
    Capture(ClipboardSnapshot),
    Search(SearchQuery),
    AiSearch(AiSearchQuery),
    PrepareSemanticIndex {
        request_id: u64,
        cursor: crate::semantic::IndexCursor,
    },
    StoreEmbeddings {
        request_id: u64,
        profile: String,
        items: Vec<crate::semantic::IndexedEmbedding>,
    },
    SemanticSearch(crate::semantic::SemanticSearchQuery),
    Load {
        id: String,
        request_id: u64,
    },
    SaveFavorite(FavoriteInput),
    EditClipboardMetadata(ClipboardMetadataInput),
    SemanticStatus {
        request_id: u64,
    },
    SaveSettings(Settings),
    Delete(String),
    Pin {
        id: String,
        pinned: bool,
    },
    Used(String),
    Flush,
    Shutdown,
    CancelShutdown,
}

#[derive(Clone, Debug)]
pub enum Event {
    OrganizerProgress {
        request_id: u64,
        progress: crate::organizer_display::Progress,
    },
    OrganizerMetadataUpdated(Vec<crate::organizer_display::MetadataUpdate>),
    OrganizerDetail {
        request_id: u64,
        result: Result<String, String>,
    },
    OrganizerReceipt {
        request_id: u64,
        result: Result<(), String>,
        pending_save: bool,
    },
    OrganizerWork(crate::organizer::Work),
    OrganizerState {
        request_id: u64,
        snapshot: crate::organizer::Snapshot,
    },
    OrganizerChanged,
    PathSuggestions {
        request_id: u64,
        items: Vec<crate::path_activity::PathSuggestion>,
    },
    PathActivityChanged,
    Ready(Settings),
    SemanticStatus {
        request_id: u64,
        status: SemanticIndexStatus,
    },
    SemanticStatusFailed {
        request_id: u64,
        message: String,
    },
    SemanticBatch(crate::semantic::IndexBatch),
    EmbeddingsStored {
        request_id: u64,
        accepted: usize,
    },
    Results {
        request_id: u64,
        items: Vec<ItemSummary>,
    },
    Loaded {
        request_id: u64,
        item: Option<StoredItem>,
    },
    /// Terminal read receipt; successful/missing reads use Loaded.
    LoadFailed {
        request_id: u64,
        message: String,
    },
    Changed,
    Saved(SaveStats),
    Error {
        message: String,
        exiting: bool,
    },
    ExitReady,
}

#[derive(Clone, Debug)]
pub struct EngineOptions {
    pub database_path: PathBuf,
    pub save_interval_ms: u64,
    pub max_pending_bytes: usize,
    pub max_item_bytes: usize,
}

impl EngineOptions {
    /// Start before the durability window closes, reserving five seconds at
    /// the default minute, and one twelfth of shorter test/custom windows.
    pub fn save_start_delay_ms(&self) -> u64 {
        self.save_interval_ms
            .saturating_sub(5_000.min(self.save_interval_ms / 12))
    }

    pub fn new(database_path: PathBuf) -> Self {
        Self {
            database_path,
            save_interval_ms: 60_000,
            max_pending_bytes: 16 * 1024 * 1024,
            max_item_bytes: 8 * 1024 * 1024,
        }
    }
}
