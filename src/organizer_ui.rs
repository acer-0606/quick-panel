//! Portable form parsing and bounded review lifecycle used by the native controls.
use crate::{
    ai::{AiSettings, AuthMethod, Effort},
    ai_models::ModelInfo,
    organizer::*,
};
use std::collections::{HashMap, HashSet};

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
pub fn effort_warning(models: &[ModelInfo], model: &str, effort: Effort) -> Option<String> {
    models.iter().find(|m| m.id == model).and_then(|m| {
        (m.supports_effort(effort) == Some(false))
            .then(|| "服务报告该模型不支持此推理强度；请选择默认或支持的强度".into())
    })
}
pub fn organizer_config_warning(ai: &AiSettings) -> Option<String> {
    if ai.model.trim().is_empty()
        || ai.encrypted_key.is_empty()
        || crate::claude::parse_endpoint(&ai.endpoint).is_err()
    {
        Some("请先在 AI 设置中填写有效 API 地址、模型 ID 和密钥；管家开关与 AI 检索独立".into())
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
    if values.len() != 6 {
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
    drafts: HashMap<Source, (String, Option<String>, Vec<String>)>,
    decision: Option<u64>,
    needs_save: bool,
    detail_request: Option<(u64, Source, String, u64)>,
    detail: Option<(Source, String, Result<String, String>)>,
    closed: bool,
    detail_epoch: u64,
    preserve_failed_drafts: bool,
}
impl ReviewState {
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
        let p = &self.snapshot.proposals[index];
        self.detail
            .as_ref()
            .filter(|(source, version, _)| {
                source == &p.candidate.source && version == &p.candidate.expected_version
            })
            .map(|(_, _, result)| result)
    }
    pub fn select(&mut self, index: Option<usize>) {
        self.selected_source = index
            .and_then(|i| self.snapshot.proposals.get(i))
            .map(|p| p.candidate.source.clone());
    }
    pub fn selected_index(&self) -> Option<usize> {
        self.snapshot
            .proposals
            .iter()
            .position(|p| Some(&p.candidate.source) == self.selected_source.as_ref())
    }
    pub fn selected(&self) -> Option<Proposal> {
        self.selected_index()
            .map(|i| self.edited(&self.snapshot.proposals[i]))
    }
    fn edited(&self, p: &Proposal) -> Proposal {
        let mut p = p.clone();
        if let Some((_, alias, tags)) = self.drafts.get(&p.candidate.source) {
            p.alias = alias.clone();
            p.tags = tags.clone();
        }
        p
    }
    pub fn edit(&mut self, alias: String, tags: String) {
        if let Some(index) = self.selected_index() {
            let candidate = &self.snapshot.proposals[index].candidate;
            self.drafts.insert(
                candidate.source.clone(),
                (
                    candidate.expected_version.clone(),
                    if alias.trim().is_empty() {
                        None
                    } else {
                        Some(alias.trim().into())
                    },
                    tags.split([',', '，', '、'])
                        .map(str::trim)
                        .filter(|t| !t.is_empty())
                        .map(str::to_owned)
                        .collect(),
                ),
            );
        }
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
                        .is_some_and(|(version, _, _)| version != &p.candidate.expected_version)
            }) {
                return Err("来源已变化，编辑内容已保留；请核对当前上下文后修改再应用".into());
            }
            let proposals: Vec<_> = self
                .snapshot
                .proposals
                .iter()
                .filter(|p| self.checked.contains(&p.candidate.source))
                .map(|p| self.edited(p))
                .collect();
            if proposals.is_empty() {
                return Err("请先勾选要处理的建议".into());
            }
            if dismiss {
                ReviewDecision::Dismiss(proposals.into_iter().map(|p| p.candidate.source).collect())
            } else {
                for p in &proposals {
                    p.validate()?;
                }
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
                self.checked.clear();
                self.drafts.clear();
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
        self.revision += 1;
        true
    }
    pub fn status(&self, enabled: bool) -> String {
        if let Some(error) = &self.snapshot.storage_error {
            return error.clone();
        }
        if let Some(warning) = &self.config_warning {
            return warning.clone();
        }
        if !self.message.is_empty() {
            return self.message.clone();
        }
        let s = &self.snapshot;
        if s.paused {
            return "连续失败，已暂停；可点击重试".into();
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
