// LTX Mac Farm — render-farm menubar monitor.
// Polls the shared farm folder (FSEvents is unreliable over SMB, so we poll every
// 2s and diff), fires a native notification + distinct sound on each ping event,
// and keeps a tray tooltip + dashboard window live.
//
// Events (a "ping" = a job moving through the pipeline):
//   queue/*.job  new  -> 📤 sent      (a job was dispatched)        sound: Tink
//   running/*    new  -> 📥 received  (a Mac picked it up)          sound: Ping
//   done/*.ok    new  -> ✅ done      (a Mac finished a render)     sound: Glass
//   failed/*     new  -> ❌ failed                                   sound: Basso

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_notification::NotificationExt;

#[derive(Default, Serialize, Clone)]
struct Counts {
    queued: usize,
    running: usize,
    done: usize,
    failed: usize,
}

#[derive(Serialize, Clone)]
struct Event {
    kind: String,
    id: String,
    host: String,
    ts: String,
}

#[derive(Default)]
struct Farm {
    counts: Counts,
    events: Vec<Event>, // most-recent last
    root: String,
}

struct SharedState(Mutex<Farm>);

fn farm_root() -> String {
    std::env::var("FARM_ROOT").unwrap_or_else(|_| "/Volumes/RenderFarm".to_string())
}

fn now_ts() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default()
}

fn list_dir(p: &Path) -> Vec<String> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if !n.starts_with('.') {
                v.push(n);
            }
        }
    }
    v
}

// "<stamp>__<id>.job[.host.pid...]" -> "<id>"
fn parse_id(name: &str) -> String {
    let after = name.splitn(2, "__").nth(1).unwrap_or(name);
    after.split(".job").next().unwrap_or(after).to_string()
}

// "...job.<HOST>.<pid>[.ok|.rcN]" -> "<HOST>"
fn parse_host(name: &str) -> String {
    name.split(".job.")
        .nth(1)
        .and_then(|r| r.split('.').next())
        .unwrap_or("?")
        .to_string()
}

fn play(sound: &str) {
    let path = format!("/System/Library/Sounds/{}.aiff", sound);
    let _ = std::process::Command::new("afplay").arg(path).spawn();
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

fn update_tray(app: &AppHandle, c: &Counts) {
    if let Some(tray) = app.tray_by_id("main") {
        let tip = format!(
            "Render Farm — queued {}  running {}  done {}  failed {}",
            c.queued, c.running, c.done, c.failed
        );
        let _ = tray.set_tooltip(Some(&tip));
    }
}

#[tauri::command]
fn get_state(state: State<SharedState>) -> serde_json::Value {
    let f = state.0.lock().unwrap();
    serde_json::json!({
        "root": f.root,
        "counts": f.counts,
        "events": f.events.iter().rev().take(60).cloned().collect::<Vec<_>>(),
    })
}

#[tauri::command]
fn show_dashboard(app: AppHandle) {
    if let Some(w) = app.get_webview_window("dash") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn spawn_watcher(app: AppHandle) {
    std::thread::spawn(move || {
        let root = farm_root();
        {
            let st: State<SharedState> = app.state();
            st.0.lock().unwrap().root = root.clone();
        }
        let dirs = ["queue", "running", "done", "failed"];
        let mut seen: HashMap<&str, HashSet<String>> = HashMap::new();
        let mut first = true;

        loop {
            let mut counts = Counts::default();
            let mut fresh: Vec<Event> = Vec::new();

            for d in dirs {
                let p = Path::new(&root).join(d);
                let names = list_dir(&p);

                let interesting = |n: &str| match d {
                    "queue" => n.ends_with(".job"),
                    "running" => n.contains(".job."),
                    "done" => n.ends_with(".ok"),
                    "failed" => n.contains(".rc"),
                    _ => false,
                };

                let c = names.iter().filter(|n| interesting(n)).count();
                match d {
                    "queue" => counts.queued = c,
                    "running" => counts.running = c,
                    "done" => counts.done = c,
                    "failed" => counts.failed = c,
                    _ => {}
                }

                let set = seen.entry(d).or_default();
                for n in &names {
                    if !interesting(n) {
                        continue;
                    }
                    if set.insert(n.clone()) && !first {
                        let id = parse_id(n);
                        let host = parse_host(n);
                        let (kind, title, body, sound) = match d {
                            "queue" => (
                                "sent",
                                "📤 Ping sent".to_string(),
                                format!("Job “{}” queued", id),
                                "Tink",
                            ),
                            "running" => (
                                "received",
                                "📥 Ping received".to_string(),
                                format!("{} picked up “{}”", host, id),
                                "Ping",
                            ),
                            "done" => (
                                "done",
                                "✅ Render done".to_string(),
                                format!("{} finished “{}”", host, id),
                                "Glass",
                            ),
                            "failed" => (
                                "failed",
                                "❌ Render failed".to_string(),
                                format!("“{}” failed on {}", id, host),
                                "Basso",
                            ),
                            _ => continue,
                        };
                        notify(&app, &title, &body);
                        play(sound);
                        fresh.push(Event {
                            kind: kind.to_string(),
                            id,
                            host,
                            ts: now_ts(),
                        });
                    }
                }
                // let requeued/re-appearing names fire again next time
                set.retain(|n| names.contains(n));
            }

            {
                let st: State<SharedState> = app.state();
                let mut f = st.0.lock().unwrap();
                f.counts = counts.clone();
                f.events.append(&mut fresh);
                let overflow = f.events.len().saturating_sub(200);
                if overflow > 0 {
                    f.events.drain(0..overflow);
                }
            }
            update_tray(&app, &counts);
            first = false;
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .manage(SharedState(Mutex::new(Farm {
            root: farm_root(),
            ..Default::default()
        })))
        .invoke_handler(tauri::generate_handler![get_state, show_dashboard])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let show = MenuItem::with_id(app, "show", "Open dashboard", true, None::<&str>)?;
            let openf = MenuItem::with_id(app, "open_folder", "Reveal farm folder", true, None::<&str>)?;
            let sep = PredefinedMenuItem::separator(app)?;
            let quit = PredefinedMenuItem::quit(app, Some("Quit LTX Mac Farm"))?;
            let menu = Menu::with_items(app, &[&show, &openf, &sep, &quit])?;

            let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;
            let _tray = TrayIconBuilder::with_id("main")
                .icon(icon)
                .icon_as_template(true)
                .tooltip("LTX Mac Farm")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("dash") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "open_folder" => {
                        let _ = std::process::Command::new("open").arg(farm_root()).spawn();
                    }
                    _ => {}
                })
                .build(app)?;

            spawn_watcher(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running LTX Mac Farm");
}
