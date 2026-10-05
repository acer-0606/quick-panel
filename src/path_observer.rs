//! Portable admission and visit policy. No filesystem or Shell calls live here.
use crate::{
    model::ItemKind,
    path_activity::{self, ObservationSource, PathActivitySettings, PathObservation},
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbeTicket {
    pub generation: u64,
    pub foreground: Option<u64>,
    pub serial: u64,
    focus_epoch: u64,
}
#[derive(Default)]
struct Visit {
    last_counted: Option<String>,
    candidate: Option<(String, u64)>,
}
#[derive(Default)]
pub struct PathObserver {
    settings: PathActivitySettings,
    generation: u64,
    focus_epoch: u64,
    foreground: Option<u64>,
    serial: u64,
    inflight: Option<ProbeTicket>,
    windows: BTreeMap<u64, Visit>,
}
impl PathObserver {
    pub fn configure(&mut self, settings: PathActivitySettings) {
        if self.settings != settings {
            self.settings = settings;
            self.clear();
        }
    }
    /// Invalidates publication, never pretends a blocked native probe has ended.
    pub fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.windows.clear();
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn foreground(&mut self, foreground: Option<u64>) {
        if self.foreground != foreground {
            if let Some(visit) = self.foreground.and_then(|id| self.windows.get_mut(&id)) {
                visit.candidate = None;
            }
            self.foreground = foreground;
            self.focus_epoch = self.focus_epoch.wrapping_add(1);
        }
    }
    pub fn begin_probe(&mut self) -> Option<ProbeTicket> {
        if !self.settings.enabled || self.busy() {
            return None;
        }
        self.serial = self.serial.wrapping_add(1);
        let ticket = ProbeTicket {
            generation: self.generation,
            foreground: self.foreground,
            serial: self.serial,
            focus_epoch: self.focus_epoch,
        };
        self.inflight = Some(ticket);
        Some(ticket)
    }
    pub fn complete(
        &mut self,
        ticket: ProbeTicket,
        observations: Vec<PathObservation>,
        monotonic_ms: u64,
    ) -> Vec<PathObservation> {
        if self.inflight != Some(ticket) {
            return vec![];
        }
        self.inflight = None;
        if !self.settings.enabled || self.generation != ticket.generation {
            return vec![];
        }
        let mut output = Vec::new();
        let mut seen = BTreeSet::new();
        let mut valid_explorer_sample = false;
        for mut observation in observations.into_iter().take(path_activity::MAX_BATCH) {
            if observation.kind == ItemKind::Clipboard {
                continue;
            }
            let Ok(target) = path_activity::normalize_target(&observation.path) else {
                continue;
            };
            let Ok(key) = path_activity::normalize_path(&target) else {
                continue;
            };
            if path_activity::excluded(&key, &self.settings.excluded_paths) {
                continue;
            }
            match observation.source {
                ObservationSource::Explorer => {
                    if ticket.focus_epoch != self.focus_epoch
                        || ticket.foreground != self.foreground
                    {
                        continue;
                    }
                    let Some(window) = ticket.foreground else {
                        continue;
                    };
                    if observation.kind != ItemKind::Folder {
                        continue;
                    }
                    valid_explorer_sample = true;
                    if self.windows.len() >= 128
                        && !self.windows.contains_key(&window)
                        && let Some(old) = self.windows.keys().next().copied()
                    {
                        self.windows.remove(&old);
                    }
                    let visit = self.windows.entry(window).or_default();
                    if visit.last_counted.as_ref() == Some(&key) {
                        visit.candidate = None;
                        continue;
                    }
                    match &visit.candidate {
                        Some((candidate, since))
                            if candidate == &key && monotonic_ms.saturating_sub(*since) >= 1500 =>
                        {
                            visit.last_counted = Some(key.clone());
                            visit.candidate = None;
                        }
                        Some((candidate, _)) if candidate == &key => continue,
                        _ => {
                            visit.candidate = Some((key, monotonic_ms));
                            continue;
                        }
                    }
                }
                ObservationSource::RecentDocuments if self.settings.recent_documents => (),
                _ => continue,
            }
            if seen.insert(key) {
                observation.path = target;
                output.push(observation);
            }
        }
        if !valid_explorer_sample
            && ticket.focus_epoch == self.focus_epoch
            && let Some(visit) = ticket
                .foreground
                .and_then(|window| self.windows.get_mut(&window))
        {
            visit.candidate = None;
        }
        output
    }
    pub fn busy(&self) -> bool {
        self.inflight.is_some()
    }
}
#[derive(Clone, Copy, Debug)]
pub struct ShellViewIdentity {
    pub root: u64,
    pub view: u64,
    pub visible: bool,
    pub active: bool,
}
/// Deduplicate references to the same active view; reject ambiguous visible tabs.
pub fn select_active_view(foreground: u64, views: &[ShellViewIdentity]) -> Option<u64> {
    let candidates: BTreeSet<_> = views
        .iter()
        .filter(|view| view.root == foreground && view.visible && view.active && view.view != 0)
        .map(|view| view.view)
        .collect();
    (candidates.len() == 1).then(|| *candidates.first().unwrap())
}

#[derive(Clone, Debug)]
pub struct RecentLink {
    pub name: String,
    pub modified: u64,
    pub size: u64,
}
#[derive(Default)]
pub struct RecentBaseline {
    links: Option<BTreeMap<String, (u64, u64)>>,
}
impl RecentBaseline {
    pub fn changed(&mut self, links: Vec<RecentLink>) -> Vec<String> {
        let next: BTreeMap<_, _> = links
            .into_iter()
            .take(2048)
            .map(|link| (link.name, (link.modified, link.size)))
            .collect();
        let changed = self
            .links
            .as_ref()
            .map(|old| {
                next.iter()
                    .filter(|(name, stamp)| old.get(*name) != Some(*stamp))
                    .take(16)
                    .map(|(name, _)| name.clone())
                    .collect()
            })
            .unwrap_or_default();
        self.links = Some(next);
        changed
    }
    pub fn reset(&mut self) {
        self.links = None;
    }
}
pub fn parse_exclusions(text: &str) -> Result<Vec<String>, String> {
    let mut roots = Vec::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if roots.len() >= 128 {
            return Err("排除目录最多 128 条".into());
        }
        roots.push(path_activity::normalize_target(line)?);
    }
    Ok(roots)
}

/// GetPath(SLGP_RAWPATH) is bounded by MAX_PATH. Reject a full buffer,
/// environment placeholders and missing saved type information without target I/O.
pub fn recent_target(
    raw: &str,
    directory: bool,
    saved_attributes: bool,
) -> Option<(String, ItemKind)> {
    if !saved_attributes || raw.encode_utf16().count() >= 259 || raw.contains('%') {
        return None;
    }
    let target = path_activity::normalize_target(raw).ok()?;
    Some((
        target,
        if directory {
            ItemKind::Folder
        } else {
            ItemKind::Document
        },
    ))
}
