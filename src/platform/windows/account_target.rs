//! Window identity capture contains no UIA or disk access. UIA types stay on MTA.
use crate::{account_fill::*, account_fill_execution::FillError, account_vault::Secret};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::File,
    io::Read,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Instant, SystemTime},
};
use windows::{
    Win32::{
        Foundation::*,
        System::{Com::*, Ole::*, Threading::*},
        UI::{Accessibility::*, WindowsAndMessaging::*},
    },
    core::{BSTR, Interface, PWSTR},
};
use zeroize::{Zeroize, Zeroizing};

pub(crate) fn hwnd(value: u64) -> HWND {
    HWND(value as usize as *mut _)
}
struct Process(HANDLE);
impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
fn process(pid: u32) -> Result<Process, FillError> {
    unsafe {
        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
            .map(Process)
            .map_err(|_| FillError::TargetChanged)
    }
}
pub fn window_identity(handle: u64) -> Result<WindowIdentity, FillError> {
    unsafe {
        let window = hwnd(handle);
        if !IsWindow(Some(window)).as_bool() {
            return Err(FillError::TargetChanged);
        }
        let mut pid = 0;
        GetWindowThreadProcessId(window, Some(&mut pid));
        let process = process(pid)?;
        let (mut start, mut exit, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        GetProcessTimes(process.0, &mut start, &mut exit, &mut kernel, &mut user)
            .map_err(|_| FillError::TargetChanged)?;
        Ok(WindowIdentity {
            handle,
            pid,
            process_start: ((start.dwHighDateTime as u64) << 32) | start.dwLowDateTime as u64,
        })
    }
}
/// Must run before displaying the panel. Reject own windows, desktop and taskbar.
pub fn capture_foreground(own_pid: u32) -> Result<WindowIdentity, FillError> {
    capture_window(unsafe { GetForegroundWindow() }.0 as usize as u64, own_pid)
}
pub(crate) fn capture_window(handle: u64, own_pid: u32) -> Result<WindowIdentity, FillError> {
    unsafe {
        let window = hwnd(handle);
        let identity = window_identity(handle)?;
        let mut class = [0u16; 256];
        let count = GetClassNameW(window, &mut class);
        let class = String::from_utf16_lossy(&class[..count.max(0) as usize]);
        if identity.pid == own_pid
            || window == GetDesktopWindow()
            || matches!(
                class.as_str(),
                "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "Progman" | "WorkerW"
            )
        {
            return Err(FillError::TeachingRequired);
        }
        Ok(identity)
    }
}
pub fn foreground_matches(target: WindowIdentity) -> bool {
    focus_handle(target).is_some()
}
pub(crate) fn focus_handle(target: WindowIdentity) -> Option<u64> {
    unsafe {
        if GetForegroundWindow() != hwnd(target.handle)
            || window_identity(target.handle) != Ok(target)
        {
            return None;
        }
        let thread = GetWindowThreadProcessId(hwnd(target.handle), None);
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if GetGUIThreadInfo(thread, &mut info).is_err()
            || info.hwndFocus.is_invalid()
            || GetAncestor(info.hwndFocus, GA_ROOT) != hwnd(target.handle)
        {
            return None;
        }
        Some(info.hwndFocus.0 as usize as u64)
    }
}
#[derive(Default)]
pub(crate) struct Fingerprints(HashMap<String, (u64, SystemTime, [u8; 32])>);
impl Fingerprints {
    fn identify(
        &mut self,
        target: WindowIdentity,
        deadline: Instant,
    ) -> Result<AppIdentity, FillError> {
        let process = process(target.pid)?;
        let mut path = vec![0u16; 32768];
        let mut len = path.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(
                process.0,
                PROCESS_NAME_WIN32,
                PWSTR(path.as_mut_ptr()),
                &mut len,
            )
        }
        .map_err(|_| FillError::QueryFailed)?;
        let canonical = std::fs::canonicalize(String::from_utf16_lossy(&path[..len as usize]))
            .map_err(|_| FillError::QueryFailed)?;
        let key = canonical.to_string_lossy().to_lowercase();
        let mut file = File::open(&canonical).map_err(|_| FillError::QueryFailed)?;
        let meta = file.metadata().map_err(|_| FillError::QueryFailed)?;
        let modified = meta.modified().map_err(|_| FillError::QueryFailed)?;
        let fingerprint = if let Some((length, time, hash)) = self
            .0
            .get(&key)
            .filter(|(length, time, _)| *length == meta.len() && *time == modified)
        {
            let _ = (length, time);
            *hash
        } else {
            let mut hash = Sha256::new();
            let mut buf = [0u8; 65536];
            loop {
                if Instant::now() >= deadline {
                    return Err(FillError::BudgetExceeded);
                }
                let count = file.read(&mut buf).map_err(|_| FillError::QueryFailed)?;
                if count == 0 {
                    break;
                }
                hash.update(&buf[..count]);
            }
            let after = file.metadata().map_err(|_| FillError::QueryFailed)?;
            if after.len() != meta.len() || after.modified().ok() != Some(modified) {
                return Err(FillError::TargetChanged);
            }
            let hash = hash.finalize().into();
            if self.0.len() >= 64 {
                self.0.clear()
            }
            self.0.insert(key.clone(), (meta.len(), modified, hash));
            hash
        };
        Ok(AppIdentity {
            canonical_path: key,
            file_fingerprint: fingerprint,
        })
    }
}
fn text(value: windows::core::BSTR, max: usize) -> Result<Option<String>, FillError> {
    if value.len() > max {
        return Err(FillError::BudgetExceeded);
    }
    let value = value.to_string();
    if value.len() > max || value.contains('\0') {
        return Err(FillError::BudgetExceeded);
    }
    Ok((!value.trim().is_empty()).then_some(value))
}
struct Array(*mut SAFEARRAY);
impl Drop for Array {
    fn drop(&mut self) {
        unsafe {
            let _ = SafeArrayDestroy(self.0);
        }
    }
}
fn runtime_id(array: Array) -> Result<Vec<i32>, FillError> {
    unsafe {
        if array.0.is_null() || SafeArrayGetDim(array.0) != 1 || SafeArrayGetElemsize(array.0) != 4
        {
            return Err(FillError::QueryFailed);
        }
        let low = SafeArrayGetLBound(array.0, 1).map_err(|_| FillError::QueryFailed)?;
        let high = SafeArrayGetUBound(array.0, 1).map_err(|_| FillError::QueryFailed)?;
        if high < low || i64::from(high) - i64::from(low) > 64 {
            return Err(FillError::QueryFailed);
        }
        let mut result = Vec::new();
        for index in low..=high {
            let mut value = 0i32;
            SafeArrayGetElement(array.0, &index, (&mut value as *mut i32).cast())
                .map_err(|_| FillError::QueryFailed)?;
            result.push(value)
        }
        Ok(result)
    }
}
pub(crate) struct Automation {
    uia: IUIAutomation,
    tuning: IUIAutomation2,
}
impl Automation {
    pub fn new() -> Result<Self, FillError> {
        unsafe {
            let uia: IUIAutomation = CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
                .map_err(|_| FillError::QueryFailed)?;
            let tuning: IUIAutomation2 = uia.cast().map_err(|_| FillError::QueryFailed)?;
            tuning
                .SetAutoSetFocus(false)
                .map_err(|_| FillError::QueryFailed)?;
            tuning
                .SetConnectionTimeout(200)
                .map_err(|_| FillError::QueryFailed)?;
            tuning
                .SetTransactionTimeout(200)
                .map_err(|_| FillError::QueryFailed)?;
            Ok(Self { uia, tuning })
        }
    }
    #[cfg_attr(test, track_caller)]
    fn query<T>(
        &self,
        deadline: Instant,
        f: impl FnOnce() -> windows::core::Result<T>,
    ) -> Result<T, FillError> {
        #[cfg(test)]
        let started = Instant::now();
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            #[cfg(test)]
            eprintln!(
                "synthetic UIA query deadline already exhausted at {}",
                std::panic::Location::caller()
            );
            return Err(FillError::BudgetExceeded);
        };
        let timeout = remaining.as_millis().clamp(1, 200) as u32;
        unsafe {
            self.tuning
                .SetConnectionTimeout(timeout)
                .map_err(|_| FillError::QueryFailed)?;
            self.tuning
                .SetTransactionTimeout(timeout)
                .map_err(|_| FillError::QueryFailed)?;
        }
        let result = f();
        #[cfg(test)]
        let hresult = result.as_ref().err().map(|error| error.code());
        let result = crate::account_fill_execution::finish_query(
            result.map_err(|error| error.code().0 as u32 == UIA_E_TIMEOUT),
            Instant::now() >= deadline,
        );
        #[cfg(test)]
        if matches!(result, Err(FillError::BudgetExceeded)) {
            eprintln!(
                "synthetic UIA query budget at {} hresult={hresult:?} timeout_ms={timeout} remaining_before={remaining:?} duration={:?}",
                std::panic::Location::caller(),
                started.elapsed()
            );
        }
        result
    }
    fn identity(
        &self,
        deadline: Instant,
        element: &IUIAutomationElement,
    ) -> Result<Vec<i32>, FillError> {
        runtime_id(self.query(deadline, || unsafe { element.GetRuntimeId().map(Array) })?)
    }
    pub fn context(
        &self,
        target: WindowIdentity,
        cache: &mut Fingerprints,
        deadline: Instant,
    ) -> Result<Context, FillError> {
        if window_identity(target.handle) != Ok(target) {
            return Err(FillError::TargetChanged);
        }
        let app = cache.identify(target, deadline)?;
        let root = self.query(deadline, || unsafe {
            self.uia.ElementFromHandle(hwnd(target.handle))
        })?;
        let root_id = self.identity(deadline, &root)?;
        let executable = app.canonical_path.rsplit('\\').next().unwrap_or("");
        let browser = matches!(
            executable,
            "chrome.exe"
                | "msedge.exe"
                | "firefox.exe"
                | "brave.exe"
                | "opera.exe"
                | "vivaldi.exe"
                | "iexplore.exe"
        );
        let changed = Arc::new(AtomicBool::new(false));
        let handler: IUIAutomationStructureChangedEventHandler = StructureObserver {
            changed: changed.clone(),
        }
        .into();
        let watch = Watch {
            uia: self.uia.clone(),
            root: root.clone(),
            handler,
            changed,
        };
        self.query(deadline, || unsafe {
            self.uia
                .AddStructureChangedEventHandler(&root, TreeScope_Subtree, None, &watch.handler)
        })?;
        Ok(Context {
            password_bounds: None,
            target,
            app,
            watch,
            root,
            window_root_id: root_id.clone(),
            root_id,
            scope: SceneScope::Window,
            needs_scope: false,
            ids: HashMap::new(),
            next_id: 0,
            patterns: HashMap::new(),
            selectors: HashMap::new(),
            target_selector: None,
            origin: if browser {
                SceneOrigin::UnverifiedBrowser
            } else {
                SceneOrigin::NativeDesktop
            },
            picked_password: None,
        })
    }
    pub fn designate(&self, context: &mut Context, binding: &Binding) -> Result<(), FillError> {
        binding.validate()?;
        if binding.app != context.app {
            return Err(FillError::TargetChanged);
        }
        context.target_selector = binding
            .target
            .as_ref()
            .map(|target| target.selector.clone());
        Ok(())
    }
    pub fn point(
        &self,
        context: &mut Context,
        x: i32,
        y: i32,
        role: super::account_picker::PickRole,
        deadline: Instant,
    ) -> Result<(u64, SceneSnapshot), FillError> {
        unsafe {
            if !foreground_matches(context.target) {
                return Err(FillError::TargetChanged);
            }
            #[cfg(test)]
            eprintln!(
                "synthetic UIA point begin scope={:?} needs_scope={} remaining={:?}",
                context.scope,
                context.needs_scope,
                deadline.saturating_duration_since(Instant::now())
            );
            let hit = self.query(deadline, || self.uia.ElementFromPoint(POINT { x, y }))?;
            #[cfg(test)]
            eprintln!(
                "synthetic UIA point hit remaining={:?}",
                deadline.saturating_duration_since(Instant::now())
            );
            if self.query(deadline, || hit.CurrentProcessId())? as u32 != context.target.pid {
                return Err(FillError::TargetChanged);
            }
            let id = self.identity(deadline, &hit)?;
            // Ancestry, not PID alone, binds points to one of two windows in a process.
            let walker = self.query(deadline, || self.uia.RawViewWalker())?;
            self.validate_window_ancestry(&hit, context, &walker, deadline)?;
            let password = self.query(deadline, || hit.CurrentIsPassword())?.as_bool();
            if (role == super::account_picker::PickRole::Password) != password {
                return Err(FillError::Unsupported);
            }
            // A completely collected selected element scope is allowed for one explicit fill.
            // The retained COM root and RuntimeId prevent a rebuilt scope replacing the old one.
            if context.needs_scope {
                context.root_id = id.clone();
                context.root = hit.clone();
                context.scope = SceneScope::SelectedContext;
                context.needs_scope = false;
            }
            #[cfg(test)]
            eprintln!(
                "synthetic UIA point sample scope={:?} needs_scope={} remaining={:?}",
                context.scope,
                context.needs_scope,
                deadline.saturating_duration_since(Instant::now())
            );
            let mut snapshot = self.sample(context, deadline)?;
            let field_id = *context.ids.get(&id).ok_or(FillError::TeachingRequired)?;
            let mut selector = context
                .selectors
                .get(&field_id)
                .cloned()
                .ok_or(FillError::TeachingRequired)?;
            if role == super::account_picker::PickRole::Password
                && snapshot
                    .fields
                    .iter()
                    .filter(|f| selector.matches(&f.selector))
                    .count()
                    > 1
            {
                context.root_id = id;
                context.root = hit;
                context.scope = SceneScope::SelectedContext;
                snapshot = self.sample(context, deadline)?;
                selector = context
                    .selectors
                    .get(&field_id)
                    .cloned()
                    .ok_or(FillError::TeachingRequired)?;
            }
            match role {
                super::account_picker::PickRole::Password => {
                    let f = snapshot
                        .fields
                        .iter()
                        .find(|f| f.field_id == field_id)
                        .ok_or(FillError::Unsupported)?;
                    if !f.visible || !f.enabled || !f.writable {
                        return Err(FillError::Unsupported);
                    }
                    context.picked_password = Some(field_id);
                }
                super::account_picker::PickRole::Username => {
                    let f = snapshot
                        .fields
                        .iter()
                        .find(|f| f.field_id == field_id)
                        .ok_or(FillError::Unsupported)?;
                    if !f.visible || !f.enabled || !f.writable {
                        return Err(FillError::Unsupported);
                    }
                }
                super::account_picker::PickRole::Target => {
                    context.target_selector = Some(selector);
                    snapshot = self.sample(context, deadline)?;
                }
            }
            Ok((field_id, snapshot))
        }
    }
    fn validate_window_ancestry(
        &self,
        element: &IUIAutomationElement,
        context: &Context,
        walker: &IUIAutomationTreeWalker,
        deadline: Instant,
    ) -> Result<(), FillError> {
        validate_ancestry(
            element.clone(),
            &context.window_root_id,
            |ancestor| {
                if self.query(deadline, || unsafe { ancestor.CurrentProcessId() })? as u32
                    != context.target.pid
                {
                    return Err(FillError::TargetChanged);
                }
                self.identity(deadline, ancestor)
            },
            |ancestor| self.query(deadline, || unsafe { walker.GetParentElement(ancestor) }),
            || {
                if Instant::now() >= deadline {
                    Err(FillError::BudgetExceeded)
                } else {
                    Ok(())
                }
            },
            FillError::TargetChanged,
        )
    }
    pub fn sample(
        &self,
        context: &mut Context,
        deadline: Instant,
    ) -> Result<SceneSnapshot, FillError> {
        unsafe {
            if context.watch.changed.load(Ordering::SeqCst) {
                return Err(FillError::TargetChanged);
            }
            if window_identity(context.target.handle) != Ok(context.target)
                || self.identity(deadline, &context.root)? != context.root_id
            {
                return Err(FillError::TargetChanged);
            }
            let walker = self.query(deadline, || self.uia.RawViewWalker())?;
            if context.scope == SceneScope::SelectedContext {
                self.validate_window_ancestry(&context.root, context, &walker, deadline)?;
            }
            let mut scene = SceneSnapshot {
                complete: false,
                scope: context.scope,
                app: context.app.clone(),
                window: context.target,
                scene_key: String::new(),
                origin: context.origin,
                fields: vec![],
                targets: vec![],
                node_count: 0,
                max_depth: 0,
            };
            context.password_bounds = None;
            context.patterns.clear();
            context.selectors.clear();
            let mut structure = Sha256::new();
            if let Err(error) = self.visit(
                context,
                &walker,
                context.root.clone(),
                vec![],
                1,
                deadline,
                &mut scene,
                &mut structure,
            ) {
                #[cfg(test)]
                eprintln!(
                    "synthetic UIA visit failed error={error:?} scope={:?} nodes={} depth={} remaining={:?}",
                    context.scope,
                    scene.node_count,
                    scene.max_depth,
                    deadline.saturating_duration_since(Instant::now())
                );
                context.origin = scene.origin;
                if error == FillError::BudgetExceeded {
                    context.needs_scope = true;
                }
                return Err(error);
            }
            if Instant::now() >= deadline {
                context.needs_scope = true;
                return Err(FillError::BudgetExceeded);
            }
            if context.watch.changed.load(Ordering::SeqCst) {
                return Err(FillError::TargetChanged);
            }
            scene.scene_key = format!("{:x}", structure.finalize());
            scene.complete = true;
            scene.validate()?;
            context.origin = scene.origin;
            Ok(scene)
        }
    }
    #[allow(clippy::too_many_arguments)]
    unsafe fn visit(
        &self,
        context: &mut Context,
        walker: &IUIAutomationTreeWalker,
        element: IUIAutomationElement,
        anchors: Vec<SelectorAnchor>,
        depth: usize,
        deadline: Instant,
        scene: &mut SceneSnapshot,
        structure: &mut Sha256,
    ) -> Result<(), FillError> {
        unsafe {
            if scene.node_count >= MAX_SCENE_NODES || depth > MAX_SCENE_DEPTH {
                return Err(FillError::BudgetExceeded);
            }
            scene.node_count += 1;
            scene.max_depth = scene.max_depth.max(depth);
            let is_password = self
                .query(deadline, || element.CurrentIsPassword())?
                .as_bool();
            let control_type = self.query(deadline, || element.CurrentControlType())?.0 as u32;
            let automation_id = text(
                self.query(deadline, || element.CurrentAutomationId())?,
                MAX_SELECTOR_BYTES,
            )?;
            let class_name = text(
                self.query(deadline, || element.CurrentClassName())?,
                MAX_SELECTOR_BYTES,
            )?;
            if control_type == UIA_DocumentControlTypeId.0 as u32
                || class_name
                    .as_deref()
                    .is_some_and(|s| s.starts_with("Chrome_WidgetWin") || s == "MozillaWindowClass")
            {
                scene.origin = SceneOrigin::UnverifiedBrowser;
            }
            let selector = FieldSelector {
                automation_id: automation_id.clone(),
                control_type,
                class_name: class_name.clone(),
                anchors: anchors.clone(),
            };
            let pattern = self
                .query(deadline, || {
                    element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                })
                .ok();
            let input = is_password
                || control_type == UIA_EditControlTypeId.0 as u32
                || control_type == UIA_ComboBoxControlTypeId.0 as u32
                || pattern.is_some();
            let label = if !input && control_type == UIA_TextControlTypeId.0 as u32 {
                text(
                    self.query(deadline, || element.CurrentName())?,
                    MAX_SELECTOR_BYTES,
                )?
            } else {
                None
            };
            structure.update(
                serde_json::to_vec(&(&selector, &label)).map_err(|_| FillError::QueryFailed)?,
            );
            let rid = self.identity(deadline, &element)?;
            let id = if let Some(id) = context.ids.get(&rid) {
                *id
            } else {
                context.next_id = context
                    .next_id
                    .checked_add(1)
                    .ok_or(FillError::QueryFailed)?;
                context.ids.insert(rid, context.next_id);
                context.next_id
            };
            context.selectors.insert(id, selector.clone());
            if context
                .target_selector
                .as_ref()
                .is_some_and(|s| s.matches(&selector))
            {
                if is_password {
                    return Err(FillError::Unsupported);
                }
                let value = if let Some(pattern) = &pattern {
                    self.query(deadline, || pattern.CurrentValue())?
                } else if !input {
                    self.query(deadline, || element.CurrentName())?
                } else {
                    return Err(FillError::Unsupported);
                };
                scene.targets.push(LiveTarget {
                    selector: selector.clone(),
                    value: text(value, MAX_TARGET_BYTES)?.ok_or(FillError::TeachingRequired)?,
                    depth,
                });
            } else if input {
                let writable = match pattern {
                    Some(ref p) => !self.query(deadline, || p.CurrentIsReadOnly())?.as_bool(),
                    None => false,
                };
                let visible = !self
                    .query(deadline, || element.CurrentIsOffscreen())?
                    .as_bool();
                let enabled = self
                    .query(deadline, || element.CurrentIsEnabled())?
                    .as_bool();
                if is_password
                    && visible
                    && enabled
                    && writable
                    && self
                        .query(deadline, || element.CurrentHasKeyboardFocus())
                        .is_ok_and(|v| v.as_bool())
                    && let Ok(r) = self.query(deadline, || element.CurrentBoundingRectangle())
                    && r.right > r.left
                    && r.bottom > r.top
                {
                    context.password_bounds = Some(PasswordBounds {
                        field_id: id,
                        left: r.left,
                        top: r.top,
                        right: r.right,
                        bottom: r.bottom,
                    });
                }
                scene.fields.push(LiveField {
                    field_id: id,
                    selector: selector.clone(),
                    depth,
                    is_password,
                    visible,
                    enabled,
                    writable,
                });
                if let Some(pattern) = pattern {
                    context.patterns.insert(id, pattern);
                }
            }
            let mut child = self.query(deadline, || walk_child(walker, &element, false))?;
            let mut child_anchors = vec![SelectorAnchor {
                automation_id,
                control_type,
                class_name,
                label,
            }];
            child_anchors.extend(anchors);
            while let Some(current) = child {
                self.visit(
                    context,
                    walker,
                    current.clone(),
                    child_anchors.clone(),
                    depth + 1,
                    deadline,
                    scene,
                    structure,
                )?;
                child = self.query(deadline, || walk_child(walker, &current, true))?;
            }
            Ok(())
        }
    }
    pub fn verify_app(
        &self,
        context: &Context,
        cache: &mut Fingerprints,
        deadline: Instant,
    ) -> Result<(), FillError> {
        if cache.identify(context.target, deadline)? != context.app {
            return Err(FillError::TargetChanged);
        }
        Ok(())
    }
    /// Current UIA focus is distinct from the native GUI-thread focus HWND.
    /// Used only by ConfirmedHint, after the fresh sample and before FillGate.consume.
    pub(crate) fn focused_password(
        &self,
        context: &Context,
        snapshot: &SceneSnapshot,
        deadline: Instant,
    ) -> Result<Option<u64>, FillError> {
        let focused = self.query(deadline, || unsafe { self.uia.GetFocusedElement() })?;
        if self.query(deadline, || unsafe { focused.CurrentProcessId() })? as u32
            != context.target.pid
        {
            return Ok(None);
        }
        let runtime_id = self.identity(deadline, &focused)?;
        Ok(context.ids.get(&runtime_id).copied().filter(|id| {
            snapshot.fields.iter().any(|field| {
                field.field_id == *id
                    && field.is_password
                    && field.visible
                    && field.enabled
                    && field.writable
            })
        }))
    }
    #[allow(clippy::too_many_arguments)]
    pub fn write(
        &self,
        context: &Context,
        id: u64,
        value: &Secret,
        lease: &crate::account_fill_execution::FillLease,
        deadline: Instant,
        expected_focus: u64,
        hint: Option<&crate::account_fill_ui::HintPermit>,
    ) -> Result<(), FillError> {
        unsafe {
            // This writable pattern was acquired by the just-completed sample.
            // No provider query is inserted between FillGate.consume and SetValue.
            let pattern = context.patterns.get(&id).ok_or(FillError::Unsupported)?;
            let utf16 = Zeroizing::new(value.expose().encode_utf16().collect::<Vec<u16>>());
            let bstr = SecretBstr(BSTR::from_wide(&utf16));
            if focus_handle(context.target) != Some(expected_focus) {
                return Err(FillError::TargetChanged);
            }
            if !lease.is_current() || hint.is_some_and(|permit| !permit.current()) {
                return Err(FillError::Cancelled);
            }
            if context.watch.changed.load(Ordering::SeqCst) {
                return Err(FillError::TargetChanged);
            }
            if Instant::now() >= deadline {
                return Err(FillError::BudgetExceeded);
            }
            // SetValue failures can be ambiguous after a remote provider starts work.
            pattern
                .SetValue(&bstr.0)
                .map_err(|_| FillError::WriteUncertain)
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PasswordBounds {
    pub field_id: u64,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}
pub(crate) struct Context {
    pub password_bounds: Option<PasswordBounds>,
    pub target: WindowIdentity,
    app: AppIdentity,
    watch: Watch,
    root: IUIAutomationElement,
    root_id: Vec<i32>,
    window_root_id: Vec<i32>,
    pub scope: SceneScope,
    needs_scope: bool,
    ids: HashMap<Vec<i32>, u64>,
    next_id: u64,
    patterns: HashMap<u64, IUIAutomationValuePattern>,
    selectors: HashMap<u64, FieldSelector>,
    pub target_selector: Option<FieldSelector>,
    origin: SceneOrigin,
    pub picked_password: Option<u64>,
}

// Preserve successful-null traversal termination separately from provider errors.
unsafe fn walk_child(
    walker: &IUIAutomationTreeWalker,
    element: &IUIAutomationElement,
    sibling: bool,
) -> windows::core::Result<Option<IUIAutomationElement>> {
    unsafe {
        let mut raw = std::ptr::null_mut();
        let vtable = walker.vtable();
        let result = if sibling {
            (vtable.GetNextSiblingElement)(walker.as_raw(), element.as_raw(), &mut raw)
        } else {
            (vtable.GetFirstChildElement)(walker.as_raw(), element.as_raw(), &mut raw)
        };
        result.ok()?;
        Ok((!raw.is_null()).then(|| IUIAutomationElement::from_raw(raw)))
    }
}
struct SecretBstr(BSTR);
impl Drop for SecretBstr {
    fn drop(&mut self) {
        let length = self.0.len();
        if length != 0 {
            // Transfer unique ownership before taking a mutable view of FFI memory.
            let raw = std::mem::take(&mut self.0).into_raw();
            unsafe {
                std::slice::from_raw_parts_mut(raw.cast_mut(), length).zeroize();
                drop(BSTR::from_raw(raw));
            }
        }
    }
}

#[windows::core::implement(IUIAutomationStructureChangedEventHandler)]
struct StructureObserver {
    changed: Arc<AtomicBool>,
}
impl IUIAutomationStructureChangedEventHandler_Impl for StructureObserver_Impl {
    fn HandleStructureChangedEvent(
        &self,
        _sender: windows::core::Ref<IUIAutomationElement>,
        _change: StructureChangeType,
        _id: *const SAFEARRAY,
    ) -> windows::core::Result<()> {
        // Callback threads only signal. They never query UIA, inspect input, or touch disk.
        self.changed.store(true, Ordering::SeqCst);
        Ok(())
    }
}
struct Watch {
    uia: IUIAutomation,
    root: IUIAutomationElement,
    handler: IUIAutomationStructureChangedEventHandler,
    changed: Arc<AtomicBool>,
}
impl Drop for Watch {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .uia
                .RemoveStructureChangedEventHandler(&self.root, &self.handler);
        }
    }
}
