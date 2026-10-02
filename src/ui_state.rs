//! Portable panel interaction and shutdown state, independent from Win32.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    Running,
    Draining,
    Saving,
    SaveFailed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelKey {
    Enter,
    Escape,
    Up,
    Down,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    NativeInput,
    Activate,
    Hide,
    SelectPrevious,
    SelectNext,
}
#[derive(Debug)]
pub struct UiState {
    pub visible: bool,
    pub composing: bool,
    pub lifecycle: Lifecycle,
    pub modal_open: bool,
    interaction_epoch: u64,
    pending_action: Option<(u64, u64)>,
    ids: Vec<String>,
    selected: Option<String>,
    request_id: u64,
    accepted_captures: usize,
}
impl Default for UiState {
    fn default() -> Self {
        Self {
            visible: false,
            composing: false,
            lifecycle: Lifecycle::Running,
            modal_open: false,
            interaction_epoch: 0,
            pending_action: None,
            ids: Vec::new(),
            selected: None,
            request_id: 0,
            accepted_captures: 0,
        }
    }
}
impl UiState {
    pub fn replace_ids(&mut self, ids: Vec<String>) {
        let previous = self.selected_index().unwrap_or(0);
        if !self.selected.as_ref().is_some_and(|id| ids.contains(id)) {
            self.selected = ids.get(previous.min(ids.len().saturating_sub(1))).cloned();
        }
        self.ids = ids;
    }
    pub fn select(&mut self, index: usize) {
        self.selected = self.ids.get(index).cloned();
    }
    pub fn selected_id(&self) -> Option<&str> {
        self.selected.as_deref()
    }
    pub fn selected_index(&self) -> Option<usize> {
        self.selected
            .as_ref()
            .and_then(|id| self.ids.iter().position(|v| v == id))
    }
    pub fn key(&self, key: PanelKey) -> KeyAction {
        if self.composing {
            return KeyAction::NativeInput;
        }
        match key {
            PanelKey::Enter => KeyAction::Activate,
            PanelKey::Escape => KeyAction::Hide,
            PanelKey::Up => KeyAction::SelectPrevious,
            PanelKey::Down => KeyAction::SelectNext,
        }
    }
    pub fn begin_action(&mut self, request_id: u64) -> bool {
        if !self.visible || self.lifecycle != Lifecycle::Running || self.modal_open {
            return false;
        }
        self.pending_action = Some((request_id, self.interaction_epoch));
        true
    }
    pub fn finish_action(&mut self, request_id: u64) -> bool {
        if !self.visible || self.lifecycle != Lifecycle::Running || self.modal_open {
            return false;
        }
        if self.pending_action != Some((request_id, self.interaction_epoch)) {
            return false;
        }
        self.pending_action = None;
        true
    }
    pub fn target_changed(&mut self) {
        self.interaction_epoch = self.interaction_epoch.wrapping_add(1);
        self.pending_action = None;
    }
    pub fn menu_cancel_should_hide(
        &self,
        command_selected: bool,
        foreground_in_panel: bool,
        foreground_in_owned_modal: bool,
    ) -> bool {
        self.visible && !command_selected && !foreground_in_panel && !foreground_in_owned_modal
    }
    pub fn may_update_target(&self, own_window: bool) -> bool {
        self.lifecycle == Lifecycle::Running && !self.modal_open && !own_window
    }
    pub fn hide(&mut self) {
        self.target_changed();
        self.visible = false;
    }
    pub fn next_request(&mut self) -> u64 {
        self.request_id = self.request_id.wrapping_add(1);
        self.request_id
    }
    pub fn is_current(&self, request_id: u64) -> bool {
        self.request_id == request_id
    }
    pub fn accept_capture(&mut self) -> bool {
        if self.lifecycle != Lifecycle::Running {
            return false;
        }
        self.accepted_captures += 1;
        true
    }
    pub fn begin_exit(&mut self) -> bool {
        if self.lifecycle != Lifecycle::Running {
            return false;
        }
        self.target_changed();
        self.lifecycle = Lifecycle::Draining;
        self.advance_exit()
    }
    fn advance_exit(&mut self) -> bool {
        if self.lifecycle == Lifecycle::Draining && self.accepted_captures == 0 {
            self.lifecycle = Lifecycle::Saving;
            true
        } else {
            false
        }
    }
    pub fn capture_completed(&mut self) -> bool {
        self.accepted_captures = self.accepted_captures.saturating_sub(1);
        self.advance_exit()
    }
    pub fn exit_failed(&mut self) {
        self.lifecycle = Lifecycle::SaveFailed;
    }
    pub fn retry_exit(&mut self) {
        self.lifecycle = Lifecycle::Saving;
    }
    pub fn cancel_exit(&mut self) {
        self.lifecycle = Lifecycle::Running;
    }
}
