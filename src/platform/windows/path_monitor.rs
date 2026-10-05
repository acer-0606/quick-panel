//! Optional external Explorer / Recent observation. Target paths are never probed.
use super::dialogs::wide;
use crate::{
    background_job::BackgroundWorker,
    model::ItemKind,
    path_activity::{self, ObservationSource, PathActivitySettings, PathObservation},
    path_observer::{
        PathObserver, ProbeTicket, RecentBaseline, RecentLink, RecentRecovery, ShellViewIdentity,
        WatcherTicket, select_active_view,
    },
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use windows::{
    Win32::{
        Foundation::*,
        Storage::FileSystem::*,
        System::{Com::*, Threading::WaitForSingleObject, Variant::VARIANT},
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::{IUnknown, Interface, PCWSTR},
};

struct Apartment;
impl Apartment {
    fn enter() -> Result<Self, String> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                .ok()
                .map_err(|e| e.to_string())?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
struct Pidl(*mut Common::ITEMIDLIST);
impl Drop for Pidl {
    fn drop(&mut self) {
        unsafe {
            CoTaskMemFree(Some(self.0.cast()));
        }
    }
}
struct ChangeWatch(usize);
impl ChangeWatch {
    fn handle(&self) -> HANDLE {
        HANDLE(self.0 as *mut _)
    }
}
impl Drop for ChangeWatch {
    fn drop(&mut self) {
        unsafe {
            let _ = FindCloseChangeNotification(self.handle());
        }
    }
}
struct Probe {
    ticket: ProbeTicket,
    settings: PathActivitySettings,
    recent_dir: Option<PathBuf>,
    initialize_recent: bool,
    recent_attempt: Option<WatcherTicket>,
    recent_revision: u64,
}
struct Receipt {
    ticket: ProbeTicket,
    observations: Vec<PathObservation>,
    error: Option<String>,
    recent_setup: Option<(PathBuf, ChangeWatch)>,
    recent_attempt: Option<WatcherTicket>,
    recent_revision: u64,
}
/// One bounded worker for the monitor's entire lifetime. Reconfiguration retains it.
pub struct PathMonitor {
    policy: PathObserver,
    settings: PathActivitySettings,
    worker: BackgroundWorker<Probe, Receipt>,
    started: Instant,
    watcher: Option<ChangeWatch>,
    recent_dir: Option<PathBuf>,
    recent_pending: bool,
    recent_recovery: RecentRecovery,
    status: Option<String>,
    last_error: Option<String>,
    cancellation: Arc<AtomicU64>,
}
impl PathMonitor {
    pub fn new(notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        let mut baseline = RecentBaseline::default();
        let mut generation = None;
        let cancellation = Arc::new(AtomicU64::new(0));
        let worker_cancellation = cancellation.clone();
        let worker = BackgroundWorker::new(
            move |probe: Probe| {
                let mut observations = Vec::new();
                let mut error = None;
                let mut recent_setup = None;
                let cancelled =
                    || worker_cancellation.load(Ordering::Acquire) != probe.ticket.generation;
                if cancelled() {
                    return Receipt {
                        ticket: probe.ticket,
                        recent_attempt: probe.recent_attempt,
                        recent_revision: probe.recent_revision,
                        observations,
                        error,
                        recent_setup,
                    };
                }
                let recent_generation = (probe.ticket.generation, probe.recent_revision);
                if generation != Some(recent_generation) || probe.initialize_recent {
                    generation = Some(recent_generation);
                    // Rebuilding a notifier always starts a fresh metadata-only baseline,
                    // including same-generation retries after an unavailable directory.
                    baseline.reset();
                }
                match Apartment::enter() {
                    Ok(_apartment) => {
                        if let Some(root) = probe.ticket.foreground {
                            match explorer_path(root, &cancelled) {
                                Ok(Some(path)) => observations.push(PathObservation {
                                    path,
                                    kind: ItemKind::Folder,
                                    observed_at_ms: now_ms(),
                                    source: ObservationSource::Explorer,
                                }),
                                Ok(None) => (),
                                Err(message) => error = Some(message),
                            }
                        }
                        if cancelled() {
                            return Receipt {
                                ticket: probe.ticket,
                                recent_attempt: probe.recent_attempt,
                                recent_revision: probe.recent_revision,
                                observations: vec![],
                                error: None,
                                recent_setup: None,
                            };
                        }
                        let recent_dir = if probe.initialize_recent {
                            match recent_directory().and_then(|dir| {
                                let name = wide(&dir.to_string_lossy());
                                let handle = unsafe {
                                    FindFirstChangeNotificationW(
                                        PCWSTR(name.as_ptr()),
                                        false,
                                        FILE_NOTIFY_CHANGE_FILE_NAME
                                            | FILE_NOTIFY_CHANGE_LAST_WRITE
                                            | FILE_NOTIFY_CHANGE_SIZE,
                                    )
                                }
                                .map_err(|e| e.to_string())?;
                                Ok((dir, ChangeWatch(handle.0 as usize)))
                            }) {
                                Ok((dir, watcher)) => {
                                    recent_setup = Some((dir.clone(), watcher));
                                    Some(dir)
                                }
                                Err(message) => {
                                    error = Some(message);
                                    None
                                }
                            }
                        } else {
                            probe.recent_dir
                        };
                        if probe.settings.recent_documents
                            && let Some(dir) = recent_dir
                        {
                            match recent_links(&dir, &mut baseline, &cancelled) {
                                Ok(paths) => observations.extend(paths),
                                Err(message) => error = Some(message),
                            }
                        }
                    }
                    Err(message) => error = Some(message),
                }
                if cancelled() {
                    return Receipt {
                        ticket: probe.ticket,
                        recent_attempt: probe.recent_attempt,
                        recent_revision: probe.recent_revision,
                        observations: vec![],
                        error: None,
                        recent_setup: None,
                    };
                }
                // Same producer gate as the engine, before queueing any observations.
                observations.retain(|observation| {
                    path_activity::normalize_path(&observation.path).is_ok_and(|key| {
                        !path_activity::excluded(&key, &probe.settings.excluded_paths)
                    })
                });
                Receipt {
                    ticket: probe.ticket,
                    recent_attempt: probe.recent_attempt,
                    recent_revision: probe.recent_revision,
                    observations,
                    error,
                    recent_setup,
                }
            },
            notify,
        );
        Self {
            policy: PathObserver::default(),
            settings: PathActivitySettings::default(),
            worker,
            started: Instant::now(),
            watcher: None,
            recent_dir: None,
            recent_pending: false,
            recent_recovery: RecentRecovery::default(),
            status: None,
            last_error: None,
            cancellation,
        }
    }
    pub fn configure(&mut self, settings: PathActivitySettings) {
        if self.settings == settings {
            return;
        }
        self.settings = settings.clone();
        self.policy.configure(settings);
        self.cancellation
            .store(self.policy.generation(), Ordering::Release);
        self.reset_recent();
    }
    pub fn clear(&mut self) {
        self.policy.clear();
        self.cancellation
            .store(self.policy.generation(), Ordering::Release);
        self.reset_recent();
    }
    fn reset_recent(&mut self) {
        self.watcher = None;
        self.recent_dir = None;
        self.recent_pending = false;
        self.status = None;
        self.last_error = None;
        self.recent_recovery.reset(
            self.settings.enabled && self.settings.recent_documents,
            self.policy.generation(),
            self.elapsed_ms(),
        );
        // Initialization and all retries use the same background worker. Resetting
        // this policy never releases its actual in-flight initialization receipt.
    }
    /// Called by the optional two-second timer. Only Win32 foreground + event checks on UI.
    pub fn tick(&mut self) {
        if !self.settings.enabled {
            return;
        }
        let now = self.elapsed_ms();
        self.policy.foreground(explorer_foreground());
        if let Some(watcher) = &self.watcher
            && unsafe { WaitForSingleObject(watcher.handle(), 0) } == WAIT_OBJECT_0
        {
            self.recent_pending = true;
            if let Err(error) = unsafe { FindNextChangeNotification(watcher.handle()) } {
                self.report(format!("常用路径 Recent 通知停止：{error}"));
                self.watcher = None;
                self.recent_dir = None;
                self.recent_pending = false;
                self.recent_recovery.lost(now);
            }
        }
        if self.worker.busy() || self.policy.busy() {
            return;
        }
        // No Explorer, Recent signal or due recovery: do not invoke COM or scan.
        if explorer_foreground().is_none() && !self.recent_pending && !self.recent_recovery.due(now)
        {
            return;
        }
        let Some(ticket) = self.policy.begin_probe() else {
            return;
        };
        let recent_dir = self
            .recent_pending
            .then(|| self.recent_dir.clone())
            .flatten();
        let recent_attempt = self.recent_recovery.begin(now);
        let initialize_recent = recent_attempt.is_some();
        let probe = Probe {
            ticket,
            settings: self.settings.clone(),
            recent_dir,
            initialize_recent,
            recent_attempt,
            recent_revision: self.recent_recovery.revision(),
        };
        if self.worker.submit(probe) {
            self.recent_pending = false;
        } else {
            self.policy.complete(ticket, vec![], now);
            if let Some(attempt) = recent_attempt {
                self.recent_recovery.complete(attempt, false, now);
            }
        }
    }
    pub fn receive(&mut self) -> Vec<PathObservation> {
        let Some(mut receipt) = self.worker.try_result() else {
            return vec![];
        };
        // Cheap recheck closes the foreground change between timer and receipt.
        self.policy.foreground(explorer_foreground());
        let current =
            receipt.ticket.generation == self.policy.generation() && self.settings.enabled;
        let now = self.elapsed_ms();
        let accepted_setup = receipt.recent_attempt.is_some_and(|attempt| {
            self.recent_recovery
                .complete(attempt, receipt.recent_setup.is_some(), now)
        });
        if current
            && accepted_setup
            && let Some((dir, watcher)) = receipt.recent_setup
        {
            self.recent_dir = Some(dir);
            self.watcher = Some(watcher);
        }
        // A notifier loss can occur while a previous Recent scan is in flight.
        // Drop only its Recent samples; the same Explorer receipt remains valid.
        receipt.observations.retain(|observation| {
            observation.source != ObservationSource::RecentDocuments
                || (receipt.recent_revision == self.recent_recovery.revision()
                    && self.recent_recovery.ready())
        });
        let observations = self
            .policy
            .complete(receipt.ticket, receipt.observations, now);
        if current && let Some(message) = receipt.error {
            self.report(format!("常用路径观察暂不可用：{message}"));
        }
        observations
    }
    fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis().min(u64::MAX as u128) as u64
    }
    fn report(&mut self, message: String) {
        if self.last_error.as_ref() != Some(&message) {
            self.last_error = Some(message.clone());
            self.status = Some(message);
        }
    }
    pub fn take_status(&mut self) -> Option<String> {
        self.status.take()
    }
    pub fn stop(&mut self) {
        self.configure(PathActivitySettings::default());
    }
}
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
/// Never guess a path from a window title; class is only a cheap Explorer gate.
fn explorer_foreground() -> Option<u64> {
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.is_invalid() {
            return None;
        }
        let root = GetAncestor(foreground, GA_ROOT);
        let mut class = [0u16; 64];
        let length = GetClassNameW(root, &mut class);
        if length <= 0 || !IsWindowVisible(root).as_bool() {
            return None;
        }
        let class = String::from_utf16_lossy(&class[..length as usize]);
        matches!(class.as_str(), "CabinetWClass" | "ExploreWClass")
            .then_some(root.0 as usize as u64)
    }
}
fn explorer_path(root: u64, cancelled: &impl Fn() -> bool) -> Result<Option<String>, String> {
    unsafe {
        if cancelled() || explorer_foreground() != Some(root) {
            return Ok(None);
        }
        let shell: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)
            .map_err(|e| e.to_string())?;
        let count = shell.Count().map_err(|e| e.to_string())?;
        // Do not infer a unique visible view from a partially enumerated collection.
        if !(0..=128).contains(&count) {
            return Ok(None);
        }
        let mut candidates = Vec::new();
        for index in 0..count {
            if cancelled() {
                return Ok(None);
            }
            let Ok(dispatch) = shell.Item(&VARIANT::from(index)) else {
                continue;
            };
            let Ok(browser) = dispatch.cast::<IWebBrowser2>() else {
                continue;
            };
            let Ok(handle) = browser.HWND() else {
                continue;
            };
            let browser_window = HWND(handle.0 as *mut _);
            if GetAncestor(browser_window, GA_ROOT).0 as usize as u64 != root {
                continue;
            }
            let Ok(provider) = dispatch.cast::<IServiceProvider>() else {
                continue;
            };
            let Ok(shell_browser) = provider.QueryService::<IShellBrowser>(&SID_STopLevelBrowser)
            else {
                continue;
            };
            let Ok(view) = shell_browser.QueryActiveShellView() else {
                continue;
            };
            let Ok(window) = view.GetWindow() else {
                continue;
            };
            let Ok(unknown) = view.cast::<IUnknown>() else {
                continue;
            };
            let identity = ShellViewIdentity {
                root: GetAncestor(window, GA_ROOT).0 as usize as u64,
                // COM identity, not the shared Explorer HWND of multiple tabs.
                view: unknown.as_raw() as usize as u64,
                visible: IsWindowVisible(window).as_bool(),
                active: true,
            };
            candidates.push((identity, shell_browser, view));
        }
        let identities: Vec<_> = candidates
            .iter()
            .map(|(identity, _, _)| *identity)
            .collect();
        let Some(selected) = select_active_view(root, &identities) else {
            return Ok(None);
        };
        let Some((_, browser, view)) = candidates
            .iter()
            .find(|(identity, _, _)| identity.view == selected)
        else {
            return Ok(None);
        };
        let folder_view: IFolderView = view.cast().map_err(|e| e.to_string())?;
        let folder: IPersistFolder2 = folder_view.GetFolder().map_err(|e| e.to_string())?;
        let pidl = Pidl(folder.GetCurFolder().map_err(|e| e.to_string())?);
        if pidl.0.is_null() {
            return Ok(None);
        }
        let mut buffer = [0u16; 2049];
        if !SHGetPathFromIDListEx(pidl.0, &mut buffer, GPFIDL_DEFAULT).as_bool() {
            return Ok(None);
        }
        // Reject focus, navigation or tab changes that happened during the COM calls.
        let active_view = browser.QueryActiveShellView().map_err(|e| e.to_string())?;
        let active_window = active_view.GetWindow().ok();
        let active_identity = active_view
            .cast::<IUnknown>()
            .ok()
            .map(|unknown| unknown.as_raw() as usize as u64);
        let window = view.GetWindow().map_err(|e| e.to_string())?;
        let active_folder: IPersistFolder2 = active_view
            .cast::<IFolderView>()
            .and_then(|view| view.GetFolder())
            .map_err(|e| e.to_string())?;
        let active_pidl = Pidl(active_folder.GetCurFolder().map_err(|e| e.to_string())?);
        if active_pidl.0.is_null() || !ILIsEqual(pidl.0, active_pidl.0).as_bool() {
            return Ok(None);
        }
        if cancelled()
            || explorer_foreground() != Some(root)
            || active_identity != Some(selected)
            || active_window != Some(window)
            || !IsWindowVisible(window).as_bool()
            || GetAncestor(window, GA_ROOT).0 as usize as u64 != root
        {
            return Ok(None);
        }
        let Some(length) = buffer.iter().position(|value| *value == 0) else {
            return Ok(None);
        };
        let path = String::from_utf16(&buffer[..length]).map_err(|e| e.to_string())?;
        Ok(path_activity::normalize_target(&path).ok())
    }
}
fn recent_directory() -> Result<PathBuf, String> {
    unsafe {
        // DONT_VERIFY avoids probing the known folder destination for existence.
        let path = SHGetKnownFolderPath(&FOLDERID_Recent, KF_FLAG_DONT_VERIFY, None)
            .map_err(|e| e.to_string())?;
        let value = path.to_string().map_err(|e| e.to_string());
        CoTaskMemFree(Some(path.0.cast()));
        value.map(PathBuf::from)
    }
}
fn recent_links(
    dir: &std::path::Path,
    baseline: &mut RecentBaseline,
    cancelled: &impl Fn() -> bool,
) -> Result<Vec<PathObservation>, String> {
    let mut links = Vec::new();
    // Metadata only on the Recent directory and its .lnk entries, never on targets.
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            baseline.reset();
            return Err(error.to_string());
        }
    };
    for (index, entry) in entries.take(8193).enumerate() {
        if cancelled() {
            baseline.reset();
            return Ok(vec![]);
        }
        if index == 8192 {
            baseline.reset();
            return Err("Recent 目录超过扫描上限，跳过本次增量".into());
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                baseline.reset();
                return Err(error.to_string());
            }
        };
        if !entry
            .path()
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
        {
            continue;
        }
        if links.len() >= 2048 {
            baseline.reset();
            return Err("Recent 超过 2048 条，跳过本次增量以避免重放旧链接".into());
        }
        // Do not follow Recent reparse points to arbitrary files or network targets.
        let file_type = match entry.file_type() {
            Ok(kind) => kind,
            Err(error) => {
                baseline.reset();
                return Err(error.to_string());
            }
        };
        if file_type.is_symlink() || !file_type.is_file() {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                baseline.reset();
                return Err(error.to_string());
            }
        };
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
            || metadata.len() > 1024 * 1024
        {
            continue;
        }
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos().min(u64::MAX as u128) as u64)
            .unwrap_or(0);
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        links.push(RecentLink {
            name,
            modified,
            size: metadata.len(),
        });
    }
    let mut observations = Vec::new();
    for name in baseline.changed(links) {
        if cancelled() {
            baseline.reset();
            return Ok(vec![]);
        }
        if let Some((path, kind)) = parse_link(&dir.join(name)) {
            observations.push(PathObservation {
                path,
                kind,
                observed_at_ms: now_ms(),
                source: ObservationSource::RecentDocuments,
            });
        }
    }
    Ok(observations)
}
fn parse_link(link_path: &std::path::Path) -> Option<(String, ItemKind)> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;
        let name = wide(&link_path.to_string_lossy());
        file.Load(PCWSTR(name.as_ptr()), STGM_READ).ok()?;
        let mut buffer = [0u16; 260];
        let mut data = WIN32_FIND_DATAW::default();
        // Saved raw target only: no Resolve(), target metadata or network existence check.
        link.GetPath(&mut buffer, &mut data, SLGP_RAWPATH.0 as u32)
            .ok()?;
        let length = buffer.iter().position(|value| *value == 0)?;
        let target = String::from_utf16(&buffer[..length]).ok()?;
        crate::path_observer::recent_target(
            &target,
            data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0,
            !matches!(data.dwFileAttributes, 0 | u32::MAX),
        )
    }
}
