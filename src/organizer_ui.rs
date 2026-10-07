//! Portable form parsing and bounded review lifecycle used by the native controls.
use crate::{
    ai::{AiSettings, AuthMethod, Effort},
    ai_models::ModelInfo,
    organizer::*,
};
use std::collections::{HashMap, HashSet};

pub fn organizer_settings_changed(
    old: &crate::model::Settings,
    next: &crate::model::Settings,
) -> bool {
    old.ai != next.ai
        || old.organizer != next.organizer
        || old.path_activity != next.path_activity
        || old.excluded_apps != next.excluded_apps
}
#[derive(Default)]
pub struct OrganizerSession {
    active: Option<(u64, u64)>,
}
impl OrganizerSession {
    pub fn begin(&mut self, id: u64, token: u64) {
        self.active = Some((id, token));
    }
    pub fn finish(&mut self, id: u64) -> Option<u64> {
        if self.active.map(|active| active.0) != Some(id) {
            return None;
        }
        self.active.take().map(|active| active.1)
    }
    pub fn cancel(&mut self) -> Option<u64> {
        self.active.take().map(|active| active.0)
    }
    pub fn settings_changed(
        &mut self,
        old: &crate::model::Settings,
        next: &crate::model::Settings,
    ) -> Option<u64> {
        if organizer_settings_changed(old, next) {
            self.cancel()
        } else {
            None
        }
    }
}
#[derive(Default)]
pub struct EditorRefresh {
    shown: Option<(Source, String)>,
    deferred: bool,
    compositions: u8,
}
impl EditorRefresh {
    pub fn replace(&mut self, candidate: Option<&Candidate>, composing: bool) -> bool {
        let identity = candidate.map(|c| (c.source.clone(), c.expected_version.clone()));
        if identity == self.shown {
            self.deferred = false;
            return false;
        }
        if composing || self.compositions != 0 {
            self.deferred = true;
            return false;
        }
        self.shown = identity;
        self.deferred = false;
        true
    }
    pub fn deferred(&self) -> bool {
        self.deferred
    }
    pub fn shown(&self) -> Option<&(Source, String)> {
        self.shown.as_ref()
    }
    pub fn composition(&mut self, editor: usize, active: bool) {
        let bit = match editor {
            0 => 1,
            1 => 2,
            _ => 0,
        };
        if active {
            self.compositions |= bit;
        } else {
            self.compositions &= !bit;
        }
    }
    pub fn composing(&self) -> bool {
        self.compositions != 0
    }
}

pub fn request_worker<Request: Send + 'static, Value: Send + 'static>(
    run: impl Fn(Request) -> Result<Value, String> + Send + Sync + 'static,
    notify: std::sync::Arc<dyn Fn() + Send + Sync>,
) -> crate::connection_test::ConnectionTestWorker<Request, Value> {
    crate::connection_test::ConnectionTestWorker::new(
        move |request| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(request)))
                .unwrap_or_else(|_| Err("后台请求失败，可稍后重试".into()))
        },
        notify,
    )
}

pub const EFFORTS: [Effort; 6] = [
    Effort::Default,
    Effort::Low,
    Effort::Medium,
    Effort::High,
    Effort::XHigh,
    Effort::Max,
];
pub fn parse_ai_fields(saved: &AiSettings, values: &[String]) -> Result<AiSettings, String> {
    if values.len() != 7 {
        return Err("AI 配置字段不完整".into());
    }
    let mut next = saved.clone();
    next.enabled = values[0] == "1";
    next.endpoint = values[1].trim().into();
    next.model = values[2].trim().into();
    next.auth = match values[3].as_str() {
        "0" => AuthMethod::ApiKey,
        "1" => AuthMethod::Bearer,
        _ => return Err("请选择认证方式".into()),
    };
    next.effort = *EFFORTS
        .get(values[6].parse::<usize>().map_err(|_| "请选择推理强度")?)
        .ok_or("请选择推理强度")?;
    if values[5] == "1" {
        if !values[4].is_empty() {
            return Err("清除密钥与填写新密钥请选择其中一个操作".into());
        }
        next.encrypted_key.clear();
        next.enabled = false;
    }
    if !values[4].is_empty()
        && (values[4].len() > 4096 || !values[4].bytes().all(|b| (33..=126).contains(&b)))
    {
        return Err("API Key 格式无效".into());
    }
    if !next.endpoint.is_empty() {
        crate::claude::parse_endpoint(&next.endpoint)?;
    }
    if next.enabled
        && (next.model.is_empty() || (values[4].is_empty() && next.encrypted_key.is_empty()))
    {
        return Err("请填写模型 ID 和 API Key".into());
    }
    Ok(next)
}
/// Local save validation shared by the native AI form.
pub fn validate_ai_form(
    saved: &AiSettings,
    values: &[String],
    models: &[ModelInfo],
) -> Result<AiSettings, String> {
    let next = parse_ai_fields(saved, values)?;
    if next.enabled
        && let Some(error) = effort_warning(models, &next.model, next.effort)
    {
        return Err(error);
    }
    Ok(next)
}
pub fn effort_warning(models: &[ModelInfo], model: &str, effort: Effort) -> Option<String> {
    models.iter().find(|m| m.id == model).and_then(|m| {
        (m.supports_effort(effort) == Some(false))
            .then(|| "服务报告该模型不支持此推理强度；请选择默认或支持的强度".into())
    })
}
pub fn organizer_config_warning(ai: &AiSettings) -> Option<String> {
    if crate::claude::validate_model(&ai.model).is_err()
        || ai.encrypted_key.is_empty()
        || crate::claude::parse_endpoint(&ai.endpoint).is_err()
    {
        Some("请先在设置中心 → 服务连接 → Claude 中填写有效 API 地址、模型 ID 和密钥；管家开关与 AI 检索独立".into())
    } else {
        None
    }
}
pub struct FormFit {
    pub width: i32,
    pub height: i32,
    pub content_height: i32,
    pub viewport_height: i32,
}
pub fn fit_form(work_width: i32, work_height: i32, dpi: u32, logical_height: i32) -> FormFit {
    let scale = |n: i32| n * dpi as i32 / 96;
    let height = scale(logical_height).min((work_height - 24).max(1));
    FormFit {
        width: scale(520).min((work_width - 24).max(1)),
        height,
        content_height: scale(logical_height - 80),
        viewport_height: (height - scale(80)).max(1),
    }
}
pub fn clamp_scroll(position: i32, content_height: i32, viewport_height: i32) -> i32 {
    position.clamp(0, (content_height - viewport_height).max(0))
}
pub fn parse_organizer_fields(values: &[String]) -> Result<OrganizerSettings, String> {
    if values.len() != 8 {
        return Err("管家配置字段不完整".into());
    }
    let settings = OrganizerSettings {
        enabled: values[0] == "1",
        mode: match values[1].as_str() {
            "0" => OrganizerMode::Suggest,
            "1" => OrganizerMode::Automatic,
            _ => return Err("请选择整理模式".into()),
        },
        scope: match values[2].as_str() {
            "0" => OrganizerScope::SavedAndPaths,
            "1" => OrganizerScope::SavedOnly,
            "2" => OrganizerScope::PathsOnly,
            "3" => OrganizerScope::AllTextAndPaths,
            _ => return Err("请选择整理范围".into()),
        },
        interval_seconds: values[3]
            .trim()
            .parse()
            .map_err(|_| "间隔请输入 30–3600 秒")?,
        batch_size: values[4].trim().parse().map_err(|_| "每批请输入 1–8 条")?,
        instructions: values[5].trim().into(),
        memory_enabled: values[6] == "1",
        max_rounds: values[7]
            .trim()
            .parse()
            .map_err(|_| "Pi 最大轮次请输入 2–8")?,
    };
    settings.validate()?;
    Ok(settings)
}
#[derive(Clone, Debug)]
pub enum ReviewDecision {
    Apply(Vec<Proposal>),
    Dismiss(Vec<Source>),
    SavePending,
}
struct Draft {
    version: String,
    alias: String,
    tags: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewRow {
    pub source: Source,
    pub title: String,
    pub alias: String,
    pub tags: String,
}

#[derive(Default)]
pub struct ListRefresh {
    rows: Vec<ReviewRow>,
}

pub struct ListUpdate {
    pub rebuild: bool,
    pub scroll: Option<(usize, i32)>,
}
impl ListRefresh {
    pub fn replace(&mut self, rows: &[ReviewRow], top: usize, offset: i32) -> ListUpdate {
        if self.rows == rows {
            return ListUpdate {
                rebuild: false,
                scroll: None,
            };
        }
        let top = top.min(self.rows.len().saturating_sub(1));
        // Keep the visible source through reorder/insertions. If it vanished,
        // prefer its following survivor, then the nearest preceding survivor.
        let scroll = self.rows[top..]
            .iter()
            .chain(self.rows[..top].iter().rev())
            .find_map(|old| rows.iter().position(|row| row.source == old.source))
            .map(|index| (index, offset));
        self.rows = rows.to_vec();
        ListUpdate {
            rebuild: true,
            scroll,
        }
    }
}

#[derive(Default)]
pub struct ReviewState {
    pub config_warning: Option<String>,
    pub snapshot: Snapshot,
    pub message: String,
    pub revision: u64,
    next_id: u64,
    snapshot_request: Option<u64>,
    selected_source: Option<Source>,
    checked: HashSet<Source>,
    drafts: HashMap<Source, Draft>,
    editor_source: Option<Source>,
    decision: Option<u64>,
    submitted_sources: HashSet<Source>,
    needs_save: bool,
    detail_request: Option<(u64, Source, String, u64)>,
    detail: Option<(Source, String, Result<String, String>)>,
    closed: bool,
    detail_epoch: u64,
    preserve_failed_drafts: bool,
}
impl ReviewState {
    pub fn list_rows(&self) -> Vec<ReviewRow> {
        self.snapshot
            .proposals
            .iter()
            .map(|p| ReviewRow {
                source: p.candidate.source.clone(),
                title: p.candidate.title.clone(),
                alias: p.alias.clone().unwrap_or_else(|| "保留原别名".into()),
                tags: p.tags.join("、"),
            })
            .chain(self.snapshot.attention.iter().map(|a| ReviewRow {
                source: a.source.clone(),
                title: a.title.clone(),
                alias: "需处理".into(),
                tags: a.message.clone(),
            }))
            .collect()
    }
    fn id(&mut self) -> u64 {
        self.next_id = self.next_id.wrapping_add(1);
        self.next_id
    }
    pub fn request_snapshot(&mut self) -> Option<u64> {
        if self.snapshot_request.is_some() {
            return None;
        }
        let id = self.id();
        self.snapshot_request = Some(id);
        Some(id)
    }
    pub fn adopt_snapshot(&mut self, id: u64, snapshot: Snapshot) -> bool {
        if self.snapshot_request != Some(id) {
            return false;
        }
        self.snapshot_request = None;
        if self.pending() || self.needs_save || self.preserve_failed_drafts {
            return false;
        }
        self.checked.retain(|source| {
            snapshot
                .proposals
                .iter()
                .any(|p| &p.candidate.source == source)
        });
        self.drafts.retain(|source, _| {
            snapshot
                .proposals
                .iter()
                .any(|p| &p.candidate.source == source)
                || self.selected_source.as_ref() == Some(source)
                || self.editor_source.as_ref() == Some(source)
        });
        self.snapshot = snapshot;
        self.revision += 1;
        true
    }
    pub fn close(&mut self) {
        self.closed = true;
        self.detail = None;
        self.detail_epoch = self.detail_epoch.wrapping_add(1);
    }
    pub fn open(&mut self) {
        self.closed = false;
    }
    pub fn refresh(&mut self) {
        self.preserve_failed_drafts = false;
        self.message.clear();
        self.detail = None;
        self.revision += 1;
    }
    pub fn request_detail(&mut self) -> Option<(u64, Source, String)> {
        if self.closed || self.detail_request.is_some() || self.pending() {
            return None;
        }
        let p = self.selected()?;
        if p.candidate.kind == crate::model::ItemKind::Clipboard {
            return None;
        }
        if self.detail.as_ref().is_some_and(|(source, version, _)| {
            source == &p.candidate.source && version == &p.candidate.expected_version
        }) {
            return None;
        }
        let request = (self.id(), p.candidate.source, p.candidate.expected_version);
        self.detail_request = Some((
            request.0,
            request.1.clone(),
            request.2.clone(),
            self.detail_epoch,
        ));
        Some(request)
    }
    pub fn adopt_detail(&mut self, id: u64, result: Result<String, String>) -> bool {
        if self.detail_request.as_ref().map(|r| r.0) != Some(id) {
            return false;
        }
        let (_, source, version, epoch) = self.detail_request.take().unwrap();
        if self.closed
            || epoch != self.detail_epoch
            || !self.selected().is_some_and(|p| {
                p.candidate.source == source && p.candidate.expected_version == version
            })
        {
            return false;
        }
        self.detail = Some((source, version, result));
        self.revision += 1;
        true
    }
    pub fn detail_target(&self) -> Option<&Result<String, String>> {
        let index = self.selected_index()?;
        let p = self.snapshot.proposals.get(index)?;
        self.detail
            .as_ref()
            .filter(|(source, version, _)| {
                source == &p.candidate.source && version == &p.candidate.expected_version
            })
            .map(|(_, _, result)| result)
    }
    pub fn select(&mut self, index: Option<usize>) {
        self.selected_source =
            index.and_then(|i| self.list_rows().get(i).map(|r| r.source.clone()));
    }
    pub fn selected_index(&self) -> Option<usize> {
        self.list_rows()
            .iter()
            .position(|p| Some(&p.source) == self.selected_source.as_ref())
    }
    pub fn selected(&self) -> Option<Proposal> {
        self.selected_index()
            .and_then(|i| self.snapshot.proposals.get(i).cloned())
    }
    pub fn editor_text(&self) -> Option<(String, String)> {
        let index = self.selected_index()?;
        let p = self.snapshot.proposals.get(index)?;
        Some(if let Some(draft) = self.drafts.get(&p.candidate.source) {
            (draft.alias.clone(), draft.tags.clone())
        } else {
            (p.alias.clone().unwrap_or_default(), p.tags.join(", "))
        })
    }
    pub fn editor_source(&mut self, source: Option<Source>) {
        self.editor_source = source;
    }
    fn submission(&self, p: &Proposal) -> Proposal {
        let mut p = p.clone();
        if let Some(draft) = self.drafts.get(&p.candidate.source) {
            p.alias = if draft.alias.trim().is_empty() {
                None
            } else {
                Some(draft.alias.trim().into())
            };
            p.tags = draft
                .tags
                .split([',', '，', '、'])
                .map(str::trim)
                .filter(|tag| !tag.is_empty())
                .map(str::to_owned)
                .collect();
        }
        p
    }
    pub fn edit(&mut self, alias: String, tags: String) {
        if let Some(p) = self.selected() {
            let candidate = &p.candidate;
            self.edit_for(
                candidate.source.clone(),
                candidate.expected_version.clone(),
                alias,
                tags,
            );
        }
    }
    pub fn edit_for(&mut self, source: Source, version: String, alias: String, tags: String) {
        self.editor_source = Some(source.clone());
        self.drafts.insert(
            source,
            Draft {
                version,
                alias,
                tags,
            },
        );
    }
    pub fn check(&mut self, index: usize, checked: bool) {
        if let Some(p) = self.snapshot.proposals.get(index) {
            if checked {
                self.checked.insert(p.candidate.source.clone());
            } else {
                self.checked.remove(&p.candidate.source);
            }
        }
    }
    pub fn checked(&self, index: usize) -> bool {
        self.snapshot
            .proposals
            .get(index)
            .is_some_and(|p| self.checked.contains(&p.candidate.source))
    }
    pub fn pending(&self) -> bool {
        self.decision.is_some()
    }
    pub fn needs_save(&self) -> bool {
        self.needs_save
    }
    pub fn begin_decision(&mut self, dismiss: bool) -> Result<(u64, ReviewDecision), String> {
        if self.pending() {
            return Err("正在保存所选建议，请稍后".into());
        }
        if let Some(error) = &self.snapshot.storage_error {
            return Err(error.clone());
        }
        let decision = if self.needs_save {
            ReviewDecision::SavePending
        } else {
            if self.snapshot.proposals.iter().any(|p| {
                self.checked.contains(&p.candidate.source)
                    && self
                        .drafts
                        .get(&p.candidate.source)
                        .is_some_and(|draft| draft.version != p.candidate.expected_version)
            }) {
                return Err("来源已变化，编辑内容已保留；请核对当前上下文后修改再应用".into());
            }
            let proposals: Vec<_> = self
                .snapshot
                .proposals
                .iter()
                .filter(|p| self.checked.contains(&p.candidate.source))
                .map(|p| self.submission(p))
                .collect();
            if proposals.is_empty() {
                return Err("请先勾选要处理的建议".into());
            }
            let sources = proposals
                .iter()
                .map(|p| p.candidate.source.clone())
                .collect();
            if dismiss {
                self.submitted_sources = sources;
                ReviewDecision::Dismiss(proposals.into_iter().map(|p| p.candidate.source).collect())
            } else {
                for p in &proposals {
                    p.validate_manual()?;
                }
                self.submitted_sources = sources;
                ReviewDecision::Apply(proposals)
            }
        };
        let id = self.id();
        self.decision = Some(id);
        self.message = "已提交，等待保存回执…".into();
        self.revision += 1;
        Ok((id, decision))
    }
    pub fn receipt(&mut self, id: u64, result: Result<(), String>, pending_save: bool) -> bool {
        if self.decision != Some(id) {
            return false;
        }
        self.decision = None;
        self.needs_save = pending_save;
        self.preserve_failed_drafts = result.is_err();
        self.message = match result {
            Ok(()) => {
                self.checked
                    .retain(|source| !self.submitted_sources.contains(source));
                self.drafts
                    .retain(|source, _| !self.submitted_sources.contains(source));
                self.submitted_sources.clear();
                "所选建议已保存".into()
            }
            Err(error) => format!(
                "未保存：{error}{}",
                if pending_save {
                    "；请点击重试保存"
                } else {
                    "；编辑内容已保留"
                }
            ),
        };
        if !pending_save {
            self.submitted_sources.clear();
        }
        self.revision += 1;
        true
    }
    pub fn status(&self, enabled: bool) -> String {
        let mut status = self.status_inner(enabled);
        if !enabled && !status.contains("管家已关闭") {
            status = format!("管家已关闭，后台整理已停止；已有建议仍可管理。{status}");
        }
        if !self.snapshot.attention.is_empty() && !status.contains("需处理") {
            status.push_str(&format!(
                "；需处理 {} 条，可选中查看原因并重试",
                self.snapshot.attention.len()
            ));
        }
        status
    }
    fn status_inner(&self, enabled: bool) -> String {
        if !self.message.is_empty()
            && (self.pending() || self.needs_save || self.preserve_failed_drafts)
        {
            return self.message.clone();
        }
        if let Some(error) = &self.snapshot.storage_error {
            return error.clone();
        }
        if let Some(warning) = &self.config_warning {
            return warning.clone();
        }
        let live = self.live_status(enabled);
        if self.message.is_empty() {
            live
        } else {
            format!("{}；{live}", self.message)
        }
    }
    fn live_status(&self, enabled: bool) -> String {
        let s = &self.snapshot;
        if s.paused {
            return "整理已暂停，请检查下方原因；修复后可点击重试".into();
        }
        if s.retry_at_ms.is_some() {
            return format!("接口失败 {} 次，正在冷却；可点击重试", s.failures);
        }
        if s.saturated {
            return format!(
                "容量已满，请应用或忽略建议；待确认 {} 条",
                s.proposals.len()
            );
        }
        if s.in_flight {
            return format!(
                "后台正在整理；待确认 {} 条，待处理 {} 条",
                s.proposals.len(),
                s.queued
            );
        }
        if !s.attention.is_empty() && s.proposals.is_empty() && s.queued == 0 {
            return format!(
                "需处理 {} 条；可选中反馈并重做，或点击重试",
                s.attention.len()
            );
        }
        if s.proposals.is_empty() && s.queued == 0 {
            return if enabled {
                "暂无建议；启用后的新内容会在后台整理"
            } else {
                "暂无建议；可在 AI 管家设置中启用整理"
            }
            .into();
        }
        format!(
            "待确认 {} 条，待处理 {} 条{}",
            s.proposals.len(),
            s.queued,
            if enabled { "" } else { "；管家已关闭" }
        )
    }
}

/// Footer slots map directly to native controls 7–10 and memory_layout.buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryFooterAction {
    Save,
    Retry,
    Settings,
    Delete,
    Close,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryFooterButton {
    pub action: MemoryFooterAction,
    pub label: &'static str,
    pub enabled: bool,
    pub visible: bool,
}
/// In-memory draft carried through a settings round trip; never a submitted command.
#[derive(Clone, Debug)]
pub struct MemoryNavigationDraft {
    pub target: Option<crate::harness_tools::FeedbackTarget>,
    pub record: Option<crate::harness_tools::MemoryRecord>,
    pub raw_input: String,
    pub scope_choice: usize,
    pub enabled: bool,
    pub confirmed: bool,
}
/// Correlated native memory operations retain the exact submitted input for save retry.
#[derive(Default)]
pub struct MemoryUiState {
    pub snapshot: crate::harness_tools::MemorySnapshot,
    pub message: String,
    pub revision: u64,
    pub pending: Option<crate::model::Command>,
    pub saving: bool,
    pub pending_save: bool,
    pub raw_input: String,
    pub navigation_draft: Option<MemoryNavigationDraft>,
    snapshot_request: Option<u64>,
    semantic_admitted: bool,
    pending_work: Option<u64>,
}
/// RandomState uses the platform's random seed; time/PID/sequence avoid counter reuse.
/// IDs are process-independent because human receipts survive application restarts.
pub fn memory_request_id() -> u64 {
    use std::{
        hash::{BuildHasher, Hasher},
        sync::atomic::{AtomicU64, Ordering},
    };
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    h.write_u32(std::process::id());
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    h.finish().max(1)
}
impl MemoryUiState {
    pub fn footer_buttons(&self, feedback: bool) -> [MemoryFooterButton; 4] {
        use MemoryFooterAction::*;
        let idle = !self.saving;
        let editable =
            idle && !self.pending_save && (!feedback || self.snapshot.feedback_available());
        let pending = self.pending.is_some();
        let retry_label = if feedback && pending && !self.semantic_admitted {
            "重试上次提交"
        } else {
            "重试保存"
        };
        [
            MemoryFooterButton {
                action: Save,
                label: if feedback {
                    "保存反馈并重做"
                } else {
                    "保存所选规则"
                },
                enabled: editable,
                visible: true,
            },
            MemoryFooterButton {
                action: if feedback || pending { Retry } else { Settings },
                label: if feedback || pending {
                    retry_label
                } else {
                    "管家设置"
                },
                enabled: idle && (!feedback || pending),
                visible: true,
            },
            MemoryFooterButton {
                action: if feedback { Settings } else { Delete },
                label: if feedback { "管家设置" } else { "删除" },
                enabled: idle && (feedback || !self.pending_save),
                visible: true,
            },
            MemoryFooterButton {
                action: Close,
                label: "关闭",
                enabled: true,
                visible: true,
            },
        ]
    }
    pub fn footer_action(&self, feedback: bool, slot: usize) -> Option<MemoryFooterAction> {
        self.footer_buttons(feedback)
            .get(slot)
            .filter(|button| button.enabled && button.visible)
            .map(|button| button.action)
    }
    pub fn navigation_draft_for(
        &self,
        target: Option<&crate::harness_tools::FeedbackTarget>,
    ) -> Option<MemoryNavigationDraft> {
        let restore = match &self.pending {
            None => true,
            Some(crate::model::Command::HarnessFeedback { sources, .. })
                if !self.saving && !self.pending_save && !self.semantic_admitted =>
            {
                target.is_some_and(|target| sources.as_slice() == std::slice::from_ref(target))
            }
            _ => false,
        };
        self.navigation_draft
            .as_ref()
            .filter(|draft| restore && draft.target.as_ref() == target)
            .cloned()
    }
    /// Bind each not-yet-admitted attempt to the job present when sent to the actor.
    pub fn bind_work(&mut self, session: &OrganizerSession) {
        if !self.semantic_admitted {
            self.pending_work = session.active.map(|active| active.0);
        }
    }
    pub fn receipt_for_session(
        &mut self,
        id: u64,
        result: Result<(), String>,
        pending_save: bool,
        session: &mut OrganizerSession,
    ) -> Option<bool> {
        let first_admission = !self.semantic_admitted && (result.is_ok() || pending_save);
        if !self.receipt(id, result, pending_save) {
            return None;
        }
        Some(
            first_admission
                && self
                    .pending_work
                    .take()
                    .is_some_and(|token| session.finish(token).is_some()),
        )
    }
    pub fn request_snapshot(&mut self) -> Option<crate::model::Command> {
        if self.snapshot_request.is_some() {
            return None;
        }
        let request_id = memory_request_id();
        self.snapshot_request = Some(request_id);
        Some(crate::model::Command::HarnessMemorySnapshot { request_id })
    }
    pub fn adopt(&mut self, request_id: u64, snapshot: crate::harness_tools::MemorySnapshot) {
        if self.snapshot_request == Some(request_id) {
            self.snapshot_request = None;
            self.snapshot = snapshot;
            self.revision += 1;
        }
    }
    pub fn submit(
        &mut self,
        command: crate::model::Command,
    ) -> Result<crate::model::Command, String> {
        if self.saving || self.pending_save {
            return Err("请先完成当前操作的保存重试".into());
        }
        self.semantic_admitted = false;
        self.pending_work = None;
        self.pending = Some(command.clone());
        self.saving = true;
        self.message = "正在保存…".into();
        self.revision += 1;
        Ok(command)
    }
    pub fn retry(&mut self) -> Option<crate::model::Command> {
        if self.saving {
            return None;
        }
        let command = self.pending.clone()?;
        self.saving = true;
        self.message = "正在重试保存…".into();
        self.revision += 1;
        Some(command)
    }
    pub fn receipt(&mut self, id: u64, result: Result<(), String>, pending_save: bool) -> bool {
        let expected = self.pending.as_ref().and_then(|c| match c {
            crate::model::Command::HarnessFeedback { request_id, .. }
            | crate::model::Command::HarnessMemoryEdit { request_id, .. }
            | crate::model::Command::HarnessMemoryDelete { request_id, .. } => Some(*request_id),
            _ => None,
        });
        if expected != Some(id) {
            return false;
        }
        self.semantic_admitted |= result.is_ok() || pending_save;
        self.saving = false;
        self.pending_save = pending_save;
        self.message = match result {
            Ok(()) if !pending_save => {
                self.pending = None;
                self.raw_input.clear();
                self.navigation_draft = None;
                "已保存".into()
            }
            Ok(()) => "尚未落盘，请重试保存".into(),
            Err(message) => format!("未保存：{message}；输入已保留"),
        };
        self.revision += 1;
        true
    }
}
impl ReviewState {
    pub fn feedback_target(&self) -> Option<(String, crate::harness_tools::FeedbackTarget)> {
        let source = self.selected_source.as_ref()?;
        if let Some(p) = self
            .snapshot
            .proposals
            .iter()
            .find(|p| &p.candidate.source == source)
        {
            return Some((
                p.candidate.title.clone(),
                crate::harness_tools::FeedbackTarget {
                    source: source.clone(),
                    expected_version: p.candidate.expected_version.clone(),
                    rejected_alias: p.alias.clone(),
                },
            ));
        }
        self.snapshot
            .attention
            .iter()
            .find(|a| &a.source == source)
            .map(|a| {
                (
                    a.title.clone(),
                    crate::harness_tools::FeedbackTarget {
                        source: source.clone(),
                        expected_version: a.expected_version.clone(),
                        rejected_alias: None,
                    },
                )
            })
    }
}

pub fn memory_text(raw: &str) -> String {
    raw.trim().replace(['\r', '\n', '\t'], " ")
}
pub fn displayed_feedback(
    snapshot: &Snapshot,
) -> Vec<(String, crate::harness_tools::FeedbackTarget)> {
    snapshot
        .proposals
        .iter()
        .map(|p| {
            (
                p.candidate.title.clone(),
                crate::harness_tools::FeedbackTarget {
                    source: p.candidate.source.clone(),
                    expected_version: p.candidate.expected_version.clone(),
                    rejected_alias: p.alias.clone(),
                },
            )
        })
        .chain(snapshot.attention.iter().map(|a| {
            (
                a.title.clone(),
                crate::harness_tools::FeedbackTarget {
                    source: a.source.clone(),
                    expected_version: a.expected_version.clone(),
                    rejected_alias: None,
                },
            )
        }))
        .collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}
impl MemoryRect {
    pub fn bottom(self) -> i32 {
        self.y + self.height
    }
    pub fn right(self) -> i32 {
        self.x + self.width
    }
}
#[derive(Clone, Debug)]
pub struct MemoryLayout {
    pub fields: [MemoryRect; 7],
    pub buttons: [MemoryRect; 4],
    pub viewport_height: i32,
    pub content_height: i32,
}
pub fn memory_layout(width: i32, height: i32, dpi: u32, feedback: bool) -> MemoryLayout {
    let scale = |v: i32| v * dpi as i32 / 96;
    let margin = scale(14);
    let gap = scale(10);
    let button_height = scale(32);
    let rows = if width >= scale(480) { 1 } else { 2 };
    let footer_height = rows * (button_height + gap) + margin;
    let viewport_height = (height - footer_height).max(1);
    // The viewport's vertical scrollbar retains its own gutter at every DPI.
    let field_width = (width - 2 * margin - scale(20)).max(1);
    let mut fields = [MemoryRect::default(); 7];
    let mut y = gap;
    for (index, logical_height) in [48, 128, 64, 128, 28, 26, 26].into_iter().enumerate() {
        if feedback && [1, 5, 6].contains(&index) {
            continue;
        }
        let field_height = scale(logical_height).min((viewport_height - 2 * gap).max(1));
        fields[index] = MemoryRect {
            x: margin,
            y,
            width: field_width,
            height: field_height,
        };
        y += field_height + gap;
    }
    let mut buttons = [MemoryRect::default(); 4];
    if rows == 1 {
        let mut x = margin;
        for (i, width) in [148, 104, 80, 80].into_iter().enumerate() {
            buttons[i] = MemoryRect {
                x,
                y: viewport_height + gap,
                width: scale(width),
                height: button_height,
            };
            x += scale(width) + gap;
        }
    } else {
        let column = ((width - 2 * margin - gap) / 2).max(1);
        for (index, button) in buttons.iter_mut().enumerate() {
            *button = MemoryRect {
                x: margin + (index as i32 % 2) * (column + gap),
                y: viewport_height + gap + (index as i32 / 2) * (button_height + gap),
                width: column,
                height: button_height,
            };
        }
    }
    MemoryLayout {
        fields,
        buttons,
        viewport_height,
        content_height: y + margin,
    }
}
pub fn memory_focus_scroll(layout: &MemoryLayout, index: usize, position: i32) -> i32 {
    let field = layout.fields[index];
    let next = if field.y < position {
        field.y
    } else if field.bottom() > position + layout.viewport_height {
        field.bottom() - layout.viewport_height
    } else {
        position
    };
    clamp_scroll(next, layout.content_height, layout.viewport_height)
}

#[derive(Default)]
pub struct MemoryInputPolicy {
    composing: bool,
}
impl MemoryInputPolicy {
    pub fn composition(&mut self, active: bool) {
        self.composing = active;
    }
    pub fn dialog_keys(&self, string_present: bool) -> bool {
        !self.composing && !string_present
    }
}
pub fn memory_text_input(raw: &str, feedback: bool) -> Result<String, String> {
    let value = memory_text(raw);
    let limit = if feedback {
        crate::harness_tools::FEEDBACK_REASON_MAX_CHARS
    } else {
        512
    };
    if value.is_empty() || value.chars().count() > limit {
        Err(format!(
            "请填写 1–{limit} 字符的{}",
            if feedback { "反馈原因" } else { "规则" }
        ))
    } else {
        Ok(value)
    }
}

/// Physical positions for every viewport child, including partially clipped fields.
pub fn memory_scrolled_fields(layout: &MemoryLayout, position: i32) -> [MemoryRect; 7] {
    let position = clamp_scroll(position, layout.content_height, layout.viewport_height);
    layout.fields.map(|mut field| {
        field.y -= position;
        field
    })
}

impl ReviewState {
    pub fn attention_detail(&self) -> Option<String> {
        self.selected_source.as_ref().and_then(|source| self.snapshot.attention.iter().find(|a| &a.source == source)).map(|a| format!("需处理：{}\r\n来源：{}\r\n所见版本：{}\r\n原因（{}）：{}\r\n\r\n该条没有可应用的完成建议。修复配置或原因后点击“重试”；也可“反馈并重做”（需启用管家与整理记忆）。", a.title, source_label(&a.source), a.expected_version, a.code, a.message))
    }
}
pub fn memory_scope_label(
    scope: &crate::harness_tools::MemoryScope,
    snapshot: &crate::harness_tools::MemorySnapshot,
) -> String {
    use crate::harness_tools::MemoryScope;
    match scope {
        MemoryScope::Global => "全局".into(),
        MemoryScope::Directory { path } => format!("目录：{path}"),
        MemoryScope::Item { source } => format!(
            "仅此内容：{}",
            snapshot
                .source_labels
                .iter()
                .find(|(s, _)| s == source)
                .map(|(_, t)| t.as_str())
                .unwrap_or("原内容已移除")
        ),
    }
}
pub fn source_label(source: &Source) -> String {
    match source {
        Source::SavedItem { id } => format!("本地内容：{id}"),
        Source::ObservedPath { normalized_path } => format!("观察路径：{normalized_path}"),
    }
}
pub fn memory_detail(
    record: &crate::harness_tools::MemoryRecord,
    snapshot: &crate::harness_tools::MemorySnapshot,
) -> String {
    use crate::harness_tools::{MemoryOrigin, MemoryScope};
    let evidence = record
        .evidence_ids
        .iter()
        .map(|id| {
            id.strip_prefix("memory:")
                .and_then(|s| s.parse::<u64>().ok())
                .and_then(|id| snapshot.records.iter().find(|m| m.id == id))
                .map(|m| format!("{}：{}", m.kind.label(), m.text))
                .unwrap_or_else(|| format!("{id}（依据已移除）"))
        })
        .collect::<Vec<_>>()
        .join("；");
    let source = match &record.scope {
        MemoryScope::Item { source } => source_label(source),
        MemoryScope::Directory { path } => path.clone(),
        MemoryScope::Global => "覆盖所有整理来源".into(),
    };
    format!(
        "类型：{}\r\n{}\r\n来源：{}{}；{}\r\n单条配置：{}，{}\r\n{}\r\n关联别名：{}\r\n关联标签：{}\r\n规则或示例：{}\r\n依据：{}\r\n\r\n保存时将换行转为空格；扩大范围会将本条完整规则或示例用于新范围，包括拒绝示例的被拒别名。",
        record.kind.label(),
        memory_scope_label(&record.scope, snapshot),
        if record.origin == MemoryOrigin::Human {
            "人工"
        } else {
            "AI"
        },
        if record.proposed_by_ai {
            "（最初由 AI 提出）"
        } else {
            ""
        },
        source,
        if record.enabled {
            "已启用"
        } else {
            "已停用"
        },
        if record.confirmed {
            "已确认"
        } else {
            "待确认"
        },
        if snapshot.record_effective(record) {
            "当前生效：是（仅所选范围）"
        } else {
            "当前不生效（需管家与整理记忆开启，且本条为已启用、已确认的人工记忆）"
        },
        record.alias.as_deref().unwrap_or("无"),
        if record.tags.is_empty() {
            "无".into()
        } else {
            record.tags.join("、")
        },
        record.text,
        if evidence.is_empty() {
            "本地整理与反馈"
        } else {
            &evidence
        }
    )
}
pub fn memory_scope_impact(
    record: &crate::harness_tools::MemoryRecord,
    scope: &crate::harness_tools::MemoryScope,
    snapshot: &crate::harness_tools::MemorySnapshot,
) -> String {
    format!(
        "所选范围：{}\r\n将用于此范围的{}：{}\r\n{}别名：{}\r\n关联标签：{}",
        memory_scope_label(scope, snapshot),
        record.kind.label(),
        record.text,
        if record.kind == crate::harness_tools::MemoryKind::Rejected {
            "被拒"
        } else {
            "关联"
        },
        record.alias.as_deref().unwrap_or("无"),
        record.tags.join("、")
    )
}
pub fn feedback_intro(title: &str, target: &crate::harness_tools::FeedbackTarget) -> String {
    format!(
        "原名称：{title}\r\n来源：{}\r\n所见被拒名称：{}\r\n拒绝原因（1–256字符）。保存即启用此反馈，并重做当前来源；选择同目录或全局会扩大此反馈及被拒名称的影响范围。",
        source_label(&target.source),
        target
            .rejected_alias
            .as_deref()
            .unwrap_or("暂无完成建议，仅记录原因")
    )
}
