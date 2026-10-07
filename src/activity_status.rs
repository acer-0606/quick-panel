//! RAM-only display of explicitly requested work; never owns a worker slot.
use crate::model::Scope;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    Claude,
    Semantic,
}
impl Capability {
    pub fn validate(self, query: &str) -> Result<(), &'static str> {
        let (limit, message) = match self {
            Self::Claude => (2048, "Claude：请输入 1–2048 字的检索描述"),
            Self::Semantic => (4096, "语义：请输入 1–4096 字的检索描述"),
        };
        if query.trim().is_empty() || query.chars().count() > limit {
            Err(message)
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Copy)]
enum Phase {
    Processing,
    Completed,
    Failed,
}
struct QueryDisplay {
    id: u64,
    capability: Capability,
    query: String,
    scope: Scope,
    profile: String,
    phase: Phase,
}
#[derive(Default)]
pub struct QueryActivity {
    current: Option<QueryDisplay>,
}
impl QueryActivity {
    pub fn begin(
        &mut self,
        id: u64,
        capability: Capability,
        query: &str,
        scope: Scope,
        profile: &str,
    ) {
        self.current = Some(QueryDisplay {
            id,
            capability,
            query: query.into(),
            scope,
            profile: profile.into(),
            phase: Phase::Processing,
        });
    }
    /// Call only after the owning query gate accepts the actual completion.
    pub fn finish(&mut self, id: u64, capability: Capability, success: bool) -> bool {
        let Some(current) = self.current.as_mut().filter(|c| {
            c.id == id && c.capability == capability && matches!(c.phase, Phase::Processing)
        }) else {
            return false;
        };
        current.phase = if success {
            Phase::Completed
        } else {
            Phase::Failed
        };
        true
    }
    pub fn cancel(&mut self) {
        self.current = None;
    }
    pub fn cancel_capability(&mut self, capability: Capability) {
        if self
            .current
            .as_ref()
            .is_some_and(|c| c.capability == capability)
        {
            self.cancel();
        }
    }
    pub fn sync(&mut self, query: &str, scope: Scope, profile: &str, active: bool) {
        if self.current.as_ref().is_some_and(|c| {
            !active || c.query != query || c.scope != scope || c.profile != profile
        }) {
            self.cancel();
        }
    }
    pub fn label(&self) -> Option<String> {
        self.current.as_ref().map(|c| {
            let (name, settings) = match c.capability {
                Capability::Claude => ("Claude", "服务连接 → Claude"),
                Capability::Semantic => ("语义", "服务连接 → Embedding"),
            };
            match c.phase {
                Phase::Processing => format!("{name} · 处理中，本地结果仍可用"),
                Phase::Completed => format!("{name} · 已完成"),
                Phase::Failed => format!("{name} · 未完成，已保留本地结果 · 设置中心 → {settings}"),
            }
        })
    }
}
pub fn capture_action(paused: bool) -> &'static str {
    if paused {
        "恢复剪贴板记录"
    } else {
        "暂停剪贴板记录"
    }
}
pub fn panel_status(paused: bool, activity: &QueryActivity, message: &str) -> String {
    let mut parts = Vec::new();
    if paused {
        parts.push("记录已暂停".into());
    }
    if let Some(label) = activity.label() {
        parts.push(label);
    }
    if !message.is_empty() {
        parts.push(message.into());
    }
    parts.join(" · ")
}
