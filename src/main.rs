#![cfg_attr(windows, windows_subsystem = "windows")]

use quick_panel::{
    engine::Engine,
    model::{ClipboardSnapshot, Command, EngineOptions, Event, Scope, SearchQuery, Settings},
};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn main() {
    if let Err(message) = run() {
        #[cfg(windows)]
        unsafe {
            use windows::{Win32::UI::WindowsAndMessaging::*, core::PCWSTR};
            let text: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
            let title: Vec<u16> = "Quick Panel".encode_utf16().chain(Some(0)).collect();
            MessageBoxW(
                None,
                PCWSTR(text.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_ICONERROR,
            );
        }
        #[cfg(not(windows))]
        eprintln!("{message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some("--core-check") => core_check(),
        Some("--benchmark") => {
            let count = arguments
                .get(1)
                .map(|value| value.parse::<usize>())
                .transpose()
                .map_err(|_| "条目数量必须是整数".to_string())?
                .unwrap_or(10_000);
            if !(1..=100_000).contains(&count) {
                return Err("基准测试条目数量范围为 1 至 100000".into());
            }
            benchmark(count)
        }
        Some("--help") | Some("-h") => {
            println!(
                "Quick Panel\nWindows: double-click to open the native panel.\n--core-check: verify memory-first capture and safe exit\n--benchmark [10000|100000]: measure the shared search core"
            );
            Ok(())
        }
        Some(argument) => Err(format!("未知参数：{argument}")),
        None => {
            #[cfg(windows)]
            {
                quick_panel::platform::windows::run()
            }
            #[cfg(not(windows))]
            {
                println!(
                    "Quick Panel 是 Windows 原生程序。可在此平台运行 --core-check 或 --benchmark。"
                );
                Ok(())
            }
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn temporary_database(label: &str) -> Result<PathBuf, String> {
    let directory = std::env::temp_dir().join(format!(
        "quick-panel-{label}-{}-{}",
        std::process::id(),
        now_ms()
    ));
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    Ok(directory.join("quick-panel.db"))
}

fn receive_until(engine: &Engine, predicate: impl Fn(&Event) -> bool) -> Result<Event, String> {
    let until = Instant::now() + Duration::from_secs(120);
    while Instant::now() < until {
        match engine.try_recv() {
            Ok(Event::Error { message, .. }) => return Err(message),
            Ok(event) if predicate(&event) => return Ok(event),
            Ok(_) => (),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(1))
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                return Err("核心已结束，未收到预期结果".into());
            }
        }
    }
    Err("等待核心结果超时".into())
}

fn snapshot(text: String) -> ClipboardSnapshot {
    ClipboardSnapshot {
        text,
        representations: vec![],
        source_app: "core-check".into(),
        captured_at_ms: now_ms(),
    }
}

fn core_check() -> Result<(), String> {
    let path = temporary_database("check")?;
    let result = (|| {
        let engine = Engine::start(EngineOptions::new(path.clone()), Arc::new(|| {}))?;
        receive_until(&engine, |event| matches!(event, Event::Ready(_)))?;
        engine.send(Command::Capture(snapshot("尚未保存的周报内容".into())))?;
        engine.send(Command::Search(SearchQuery {
            text: "周报".into(),
            scope: Scope::All,
            request_id: 1,
        }))?;
        let visible = match receive_until(&engine, |event| {
            matches!(event, Event::Results { request_id: 1, .. })
        })? {
            Event::Results { items, .. } => items.iter().any(|item| item.title.contains("周报")),
            _ => false,
        };
        let connection = rusqlite::Connection::open(&path).map_err(|error| error.to_string())?;
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        drop(connection);
        engine.send(Command::Shutdown)?;
        receive_until(&engine, |event| matches!(event, Event::ExitReady))?;
        drop(engine);
        let restored = Engine::start(EngineOptions::new(path.clone()), Arc::new(|| {}))?;
        receive_until(&restored, |event| matches!(event, Event::Ready(_)))?;
        restored.send(Command::Search(SearchQuery {
            text: "周报".into(),
            scope: Scope::All,
            request_id: 2,
        }))?;
        let persisted = matches!(
            receive_until(&restored, |event| matches!(event, Event::Results { request_id: 2, .. }))?,
            Event::Results { items, .. } if items.iter().any(|item| item.title.contains("周报"))
        );
        restored.send(Command::Shutdown)?;
        receive_until(&restored, |event| matches!(event, Event::ExitReady))?;
        drop(restored);
        if !visible || count != 0 || !persisted {
            return Err("内存可见性或退出保存校验失败".into());
        }
        println!(
            "{}",
            serde_json::json!({
                "pending_visible":visible, "disk_empty_before_flush":count==0,
                "exit_restored":persisted, "sqlite_version":rusqlite::version()
            })
        );
        Ok(())
    })();
    if let Some(directory) = path.parent() {
        let _ = std::fs::remove_dir_all(directory);
    }
    result
}

fn benchmark(count: usize) -> Result<(), String> {
    let path = temporary_database("benchmark")?;
    let result = (|| {
        let engine = Engine::start(EngineOptions::new(path.clone()), Arc::new(|| {}))?;
        receive_until(&engine, |event| matches!(event, Event::Ready(_)))?;
        engine.send(Command::SaveSettings(Settings {
            max_history: count.max(10_000),
            ..Default::default()
        }))?;
        receive_until(&engine, |event| matches!(event, Event::Saved(_)))?;
        let seed_started = Instant::now();
        for index in 0..count {
            engine.send(Command::Capture(snapshot(format!(
                "项目{index} 接口说明 周报第{index}期 C:/Projects/project-{index}/readme.md"
            ))))?;
            // Setup deliberately flushes chunks to keep benchmark input memory bounded.
            if (index + 1) % 1_000 == 0 {
                engine.send(Command::Flush)?;
                receive_until(&engine, |event| matches!(event, Event::Saved(_)))?;
            }
        }
        engine.send(Command::Flush)?;
        engine.send(Command::Search(SearchQuery {
            request_id: 1,
            ..Default::default()
        }))?;
        receive_until(&engine, |event| {
            matches!(event, Event::Results { request_id: 1, .. })
        })?;
        let database = rusqlite::Connection::open(&path).map_err(|error| error.to_string())?;
        let stored_count: i64 = database
            .query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        let stored_count = usize::try_from(stored_count).map_err(|error| error.to_string())?;
        drop(database);
        if stored_count != count {
            return Err(format!(
                "基准数据数量不符：预期 {count}，实际 {stored_count}"
            ));
        }
        let seed_ms = seed_started.elapsed().as_millis();
        let mut latencies = Vec::new();
        for request_id in 2..102 {
            let query =
                ["接口", "报", "项目42", "jiekou", "readme", "不存在"][request_id as usize % 6];
            let started = Instant::now();
            engine.send(Command::Search(SearchQuery {
                text: query.into(),
                scope: Scope::All,
                request_id,
            }))?;
            receive_until(
                &engine,
                |event| matches!(event, Event::Results { request_id: id, .. } if *id == request_id),
            )?;
            latencies.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        latencies.sort_by(f64::total_cmp);
        engine.send(Command::Shutdown)?;
        receive_until(&engine, |event| matches!(event, Event::ExitReady))?;
        drop(engine);
        println!(
            "{}",
            serde_json::json!({
                "platform": std::env::consts::OS,
                "architecture": std::env::consts::ARCH,
                "items":stored_count, "queries":latencies.len(), "seed_ms":seed_ms,
                "search_p50_ms":latencies[49], "search_p95_ms":latencies[94],
                "search_max_ms":latencies[99],
                "measurement":"shared engine query round-trip; excludes Windows UI",
                "sqlite_version": rusqlite::version()
            })
        );
        Ok(())
    })();
    if let Some(directory) = path.parent() {
        let _ = std::fs::remove_dir_all(directory);
    }
    result
}
