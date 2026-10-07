//! Local form validation and submission policy shared with the native modal pump.
use crate::{
    ai::AiSettings,
    ai_models::ModelInfo,
    semantic::{EmbeddingSettings, IndexScope},
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormError {
    pub field: usize,
    pub message: String,
}
impl FormError {
    pub fn new(field: usize, message: impl Into<String>) -> Self {
        Self {
            field,
            message: message.into(),
        }
    }
}
#[derive(Clone)]
pub enum Validation {
    None,
    General,
    Settings(Box<crate::model::Settings>, crate::settings_ui::Action),
    Screenshot,
    Paths,
    Favorite,
    Metadata(crate::model::ItemSummary),
    Ai(AiSettings),
    Embedding(EmbeddingSettings),
    Organizer(AiSettings),
}
fn count(fields: &[String], expected: usize) -> Result<(), FormError> {
    if fields.len() == expected {
        Ok(())
    } else {
        Err(FormError::new(0, "表单字段不完整"))
    }
}
fn key(fields: &[String], password: usize, clear: usize, existing: &[u8]) -> Result<(), FormError> {
    let value = &fields[password];
    if fields[clear] == "1" && !value.is_empty() {
        return Err(FormError::new(
            clear,
            "清除密钥与填写新密钥请选择其中一个操作",
        ));
    }
    if !value.is_empty() && (value.len() > 4096 || !value.bytes().all(|b| (33..=126).contains(&b)))
    {
        return Err(FormError::new(password, "API Key 格式无效"));
    }
    if fields[0] == "1" && fields[clear] != "1" && value.is_empty() && existing.is_empty() {
        return Err(FormError::new(password, "启用服务前请填写 API Key"));
    }
    Ok(())
}
pub fn parse_embedding_fields(
    saved: &EmbeddingSettings,
    fields: &[String],
) -> Result<EmbeddingSettings, FormError> {
    count(fields, 6)?;
    let mut next = saved.clone();
    next.enabled = fields[0] == "1" && fields[4] != "1";
    next.endpoint = fields[1].trim().into();
    next.model = fields[2].trim().into();
    if next.enabled || !next.endpoint.is_empty() {
        crate::embedding::parse_endpoint(&next.endpoint).map_err(|e| FormError::new(1, e))?;
    }
    if next.enabled || !next.model.is_empty() {
        crate::embedding::validate_model(&next.model).map_err(|e| FormError::new(2, e))?;
    }
    key(fields, 3, 4, &saved.encrypted_key)?;
    next.index_scope = match fields[5].as_str() {
        "0" => IndexScope::PinnedAndFavorites,
        "1" => IndexScope::AllText,
        _ => return Err(FormError::new(5, "请选择索引范围")),
    };
    if fields[4] == "1" {
        next.encrypted_key.clear();
    }
    Ok(next)
}
pub fn validate(
    kind: &Validation,
    fields: &[String],
    models: &[ModelInfo],
) -> Result<(), FormError> {
    match kind {
        Validation::None => (),
        Validation::Settings(saved, action) => {
            crate::settings_ui::edit(saved, *action, fields)?;
        }
        Validation::General => {
            crate::settings_ui::edit(
                &crate::model::Settings::default(),
                crate::settings_ui::Action::General,
                fields,
            )?;
        }
        Validation::Screenshot => {
            count(fields, 1)?;
            crate::hotkeys::parse_hotkey(&fields[0]).map_err(|e| FormError::new(0, e))?;
        }
        Validation::Paths => {
            count(fields, 4)?;
            crate::path_observer::parse_exclusions(&fields[2]).map_err(|e| FormError::new(2, e))?;
        }
        Validation::Favorite => {
            count(fields, 5)?;
            for (i, label) in [(0, "名称"), (1, "目标路径")] {
                if fields[i].trim().is_empty() {
                    return Err(FormError::new(i, format!("{label}不能为空")));
                }
            }
        }
        Validation::Metadata(summary) => {
            count(fields, 3)?;
            let form = crate::smart_search::ClipboardMetadataForm {
                summary: summary.clone(),
            };
            form.finish(Some(fields.to_vec()), true).map_err(|e| {
                let invalid = |s: &str, max| {
                    s.trim().chars().count() > max || s.trim().chars().any(char::is_control)
                };
                let field = if fields[0].trim().is_empty() || invalid(&fields[0], 160) {
                    0
                } else if invalid(&fields[1], 128) {
                    1
                } else {
                    2
                };
                FormError::new(field, e)
            })?;
        }
        Validation::Ai(saved) => {
            count(fields, 7)?;
            let enabled = fields[0] == "1" && fields[5] != "1";
            if enabled || !fields[1].trim().is_empty() {
                crate::claude::parse_endpoint(fields[1].trim())
                    .map_err(|e| FormError::new(1, e))?;
            }
            if enabled || !fields[2].trim().is_empty() {
                crate::claude::validate_model(fields[2].trim())
                    .map_err(|e| FormError::new(2, e))?;
            }
            key(fields, 4, 5, &saved.encrypted_key)?;
            if !matches!(fields[3].as_str(), "0" | "1") {
                return Err(FormError::new(3, "请选择认证方式"));
            }
            crate::organizer_ui::validate_ai_form(saved, fields, models)
                .map_err(|e| FormError::new(6, e))?;
        }
        Validation::Embedding(saved) => {
            parse_embedding_fields(saved, fields)?;
        }
        Validation::Organizer(ai) => {
            count(fields, 8)?;
            for (i, min, max, label) in [
                (3, 30, 3600, "整理间隔"),
                (4, 1, 8, "每批数量"),
                (7, 2, 8, "Pi 最大轮次"),
            ] {
                if fields[i]
                    .trim()
                    .parse::<u32>()
                    .ok()
                    .is_none_or(|v| !(min..=max).contains(&v))
                {
                    return Err(FormError::new(i, format!("{label}请输入 {min}–{max}")));
                }
            }
            let next = crate::organizer_ui::parse_organizer_fields(fields)
                .map_err(|e| FormError::new(5, e))?;
            if next.enabled
                && let Some(error) = crate::organizer_ui::organizer_config_warning(ai)
            {
                return Err(FormError::new(0, error));
            }
        }
    }
    Ok(())
}
/// Separate channels: health polling may replace only the explicit test result.
#[derive(Default)]
pub struct FormStatus {
    pub config: String,
    pub error: Option<FormError>,
    pub test: String,
}
impl FormStatus {
    pub fn runtime_health(&mut self, message: String) {
        self.test = message;
    }
    pub fn text(&self) -> String {
        [
            self.config.clone(),
            self.error
                .as_ref()
                .map(|e| format!("未保存：{}", e.message))
                .unwrap_or_default(),
            self.test.clone(),
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\r\n")
    }
}
/// The native pump keeps its controls alive until this submission is accepted.
#[derive(Default)]
pub struct Submission {
    pub accepted: bool,
    pub error: Option<FormError>,
}
impl Submission {
    pub fn attempt(
        &mut self,
        fields: &[String],
        submit: impl FnOnce(&[String]) -> Result<(), FormError>,
    ) {
        if self.accepted {
            return;
        }
        self.error = submit(fields).err();
        self.accepted = self.error.is_none();
    }
}

/// Reports a manual-test effort rejection without retaining the form's model borrow
/// across the status callback, which may update the same form or dispatch messages.
pub fn report_effort_warning<T>(
    form: &std::cell::RefCell<T>,
    models: impl FnOnce(&T) -> &[ModelInfo],
    model: &str,
    effort: crate::ai::Effort,
    report: impl FnOnce(&str),
) -> bool {
    let message = {
        let form = form.borrow();
        crate::organizer_ui::effort_warning(models(&form), model, effort)
    };
    if let Some(message) = message {
        report(&message);
        true
    } else {
        false
    }
}
