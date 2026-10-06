//! Lightweight, memory-only organizer presentation; no content or network work.
use crate::{model::ItemSummary, organizer::Source, path_activity::PathSuggestion};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Waiting,
    Processing,
    Ready,
}
impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Waiting => "待整理",
            Self::Processing => "整理中",
            Self::Ready => "有整理建议",
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub items: Vec<(Source, Stage)>,
    pub paused: bool,
    pub cooling: bool,
    pub saturated: bool,
    pub storage_error: bool,
}
impl Progress {
    pub fn label(&self) -> String {
        let status = if self.storage_error {
            "缓存需修复"
        } else if self.paused {
            "已暂停"
        } else if self.cooling {
            "冷却中"
        } else if self.saturated {
            "待清理建议"
        } else if self.items.iter().any(|(_, s)| *s == Stage::Processing) {
            "整理中"
        } else {
            ""
        };
        let ready = self
            .items
            .iter()
            .filter(|(_, s)| *s == Stage::Ready)
            .count();
        let waiting = self
            .items
            .iter()
            .filter(|(_, s)| *s == Stage::Waiting)
            .count();
        let mut text = String::from("AI 管家");
        if !status.is_empty() {
            text.push_str(&format!(" · {status}"));
        }
        if ready > 0 {
            text.push_str(&format!(" · {ready} 条建议"));
        } else if status.is_empty() && waiting > 0 {
            text.push_str(&format!(" · 待整理 {waiting} 条"));
        } else if status.is_empty() {
            text.push_str(" · 暂无待办");
        }
        text
    }
}
#[derive(Default)]
pub struct ProgressCache {
    snapshot: Progress,
    revision: u64,
    generation: u64,
    next_id: u64,
    active: Option<(u64, u64)>,
    dirty: bool,
}
impl ProgressCache {
    pub fn snapshot(&self) -> &Progress {
        &self.snapshot
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn mark_dirty(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.dirty = true;
    }
    pub fn clear(&mut self) {
        self.mark_dirty();
        if self.snapshot != Progress::default() {
            self.snapshot = Progress::default();
            self.revision = self.revision.wrapping_add(1);
        }
        // Keep a real outstanding read occupied until its terminal receipt.
    }
    pub fn request(&mut self) -> Option<u64> {
        if !self.dirty || self.active.is_some() {
            return None;
        }
        self.next_id = self.next_id.wrapping_add(1);
        self.active = Some((self.next_id, self.generation));
        Some(self.next_id)
    }
    pub fn complete(&mut self, id: u64, snapshot: Progress) -> bool {
        let Some((active, generation)) = self.active else {
            return false;
        };
        if active != id {
            return false;
        }
        self.active = None;
        if generation != self.generation {
            return false;
        }
        self.dirty = false;
        if self.snapshot != snapshot {
            self.snapshot = snapshot;
            self.revision = self.revision.wrapping_add(1);
        }
        true
    }
}
#[derive(Clone, Debug)]
pub struct MetadataUpdate {
    pub source: Source,
    pub previous_alias: String,
    pub previous_tags: Vec<String>,
    pub alias: String,
    pub tags: Vec<String>,
}
impl MetadataUpdate {
    pub fn apply_saved(&self, item: &mut ItemSummary) -> bool {
        matches!(&self.source, Source::SavedItem { id } if *id == item.id)
            && self.apply(&mut item.alias, &mut item.tags)
    }
    pub fn apply_path(&self, path: &mut PathSuggestion) -> bool {
        matches!(&self.source, Source::ObservedPath { normalized_path } if *normalized_path == path.key)
            && self.apply(&mut path.alias, &mut path.tags)
    }
    fn apply(&self, alias: &mut String, tags: &mut Vec<String>) -> bool {
        if *alias != self.previous_alias
            || *tags != self.previous_tags
            || (*alias == self.alias && *tags == self.tags)
        {
            return false;
        }
        alias.clone_from(&self.alias);
        tags.clone_from(&self.tags);
        true
    }
}
