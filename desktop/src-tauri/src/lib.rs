// LTX Mac Farm — render-farm menubar monitor + in-app setup checker.
//
// Two halves:
//   1. WATCHER  — polls the shared farm folder (FSEvents is unreliable over SMB,
//      so we poll every 2s and diff), fires a native notification + distinct
//      sound on each ping event, keeps a tray tooltip + dashboard window live.
//   2. SETUP    — a live "Setup & Verify" view: every step from the README as a
//      check that reports ✅/⚠️/❌ for THIS Mac, with the exact fix and a button
//      that performs it. New Macs join the farm without reading the README.
//
// Events (a "ping" = a job moving through the pipeline):
//   queue/*.job  new  -> 📤 sent      (a job was dispatched)        sound: Tink
//   running/*    new  -> 📥 received  (a Mac picked it up)          sound: Ping
//   done/*.ok    new  -> ✅ done      (a Mac finished a render)     sound: Glass
//   failed/*     new  -> ❌ failed                                   sound: Basso

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_notification::NotificationExt;

// ---------------------------------------------------------------------------
// Config — persisted JSON, replaces the env-var-only FARM_ROOT.
// Resolution order everywhere: saved config -> env var -> built-in default.
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct Config {
    coordinator: String,  // the host Mac's name -> smb://<name>.local/RenderFarm
    share_path: String,   // where the share is mounted on THIS Mac
    share_name: String,   // the shared folder's name on the coordinator
    perf: String,         // this Mac's default speed profile: full | light
    min_free_gb: u64,     // worker pauses below this much free disk
    ltx_dir: String,      // local LTX2-MLX checkout
    lora_dir: String,     // local LoRA dir (provision.command fills it)
    repo_dir: String,     // where the farm scripts live on this Mac ("" = autodetect)
    role: String,         // "" (unasked) | "coordinator" | "worker"
    wizard_done: bool,    // false -> the app opens the guided setup instead of the dashboard
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/Users/Shared".to_string())
}

impl Default for Config {
    fn default() -> Self {
        Self {
            coordinator: String::new(),
            share_path: String::new(),
            share_name: "RenderFarm".to_string(),
            perf: "full".to_string(),
            min_free_gb: 15,
            ltx_dir: String::new(),
            lora_dir: format!("{}/farm-loras", home()),
            repo_dir: String::new(),
            role: String::new(),
            wizard_done: false,
        }
    }
}

impl Config {
    // config -> env -> default
    fn name(&self) -> String {
        let n = self.share_name.trim();
        if n.is_empty() { "RenderFarm".to_string() } else { n.to_string() }
    }

    // Where the farm folder lives ON THIS MAC. The two roles are NOT the same
    // path and conflating them is how you get "Permission denied (os error 13)":
    //   coordinator — it HOSTS the folder, so it's a real local dir (~/RenderFarm).
    //   worker      — it MOUNTS the coordinator's folder at /Volumes/<name>.
    // /Volumes itself is root:wheel, so a coordinator that thinks it should
    // write to /Volumes/RenderFarm cannot create anything there.
    fn root(&self) -> String {
        if !self.share_path.trim().is_empty() {
            return self.share_path.trim().to_string();
        }
        if self.role == "coordinator" {
            return self.local_root();
        }
        std::env::var("FARM_ROOT").unwrap_or_else(|_| format!("/Volumes/{}", self.name()))
    }

    fn local_root(&self) -> String {
        format!("{}/{}", home(), self.name())
    }
    fn ltx(&self) -> String {
        if !self.ltx_dir.trim().is_empty() {
            return self.ltx_dir.trim().to_string();
        }
        std::env::var("LTX_DIR").unwrap_or_else(|_| format!("{}/video-gen/LTX2-MLX", home()))
    }
    fn share_url(&self) -> String {
        let name = if self.share_name.trim().is_empty() {
            "RenderFarm"
        } else {
            self.share_name.trim()
        };
        format!("smb://{}.local/{}", safe_host(&self.coordinator), name)
    }
}

fn config_path() -> PathBuf {
    PathBuf::from(format!(
        "{}/Library/Application Support/design.aidxn.ltx-mac-farm/config.json",
        home()
    ))
}

fn load_config() -> Config {
    let mut cfg: Config = std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str::<Config>(&s).ok())
        .unwrap_or_default();
    cfg.normalize();
    cfg
}

impl Config {
    // Repair configs written before root() knew about roles. A coordinator with
    // no share_path used to resolve to /Volumes/<name>, which is root-owned, so
    // creating the queue folders failed with EACCES. Healing on load means an
    // already-broken install fixes itself on next launch — nobody has to redo
    // the wizard or hand-edit JSON.
    fn normalize(&mut self) {
        if self.role != "coordinator" {
            return;
        }
        if self.share_path.trim().starts_with("/Volumes/") {
            self.share_path = self.local_root();
        }
        if self.coordinator.trim().is_empty() {
            self.coordinator = this_host();
        }
    }
}

fn write_config(cfg: &Config) -> Result<(), String> {
    let p = config_path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&p, json).map_err(|e| e.to_string())
}

// Hostnames land in shell commands (ping / open smb://). Keep them boring.
fn safe_host(s: &str) -> String {
    s.trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '.' || *c == '_')
        .collect()
}

// ---------------------------------------------------------------------------
// Watcher state
// ---------------------------------------------------------------------------

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
struct CfgState(Mutex<Config>);

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

// same as list_dir but keeps dotfiles (worker locks live at running/.worker.<host>.lock)
fn list_dir_all(p: &Path) -> Vec<String> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            v.push(e.file_name().to_string_lossy().to_string());
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
    let _ = Command::new("afplay").arg(path).spawn();
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

// ---------------------------------------------------------------------------
// Shell helpers for the setup checks
// ---------------------------------------------------------------------------

fn sh(cmd: &str) -> String {
    Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .map(|o| {
            let mut s = String::from_utf8_lossy(&o.stdout).to_string();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            s
        })
        .unwrap_or_default()
}

// A GUI app inherits a bare PATH, so ask a login shell where a tool is.
fn which_login(bin: &str) -> Option<String> {
    let out = sh(&format!("/bin/zsh -lc 'command -v {}' 2>/dev/null", bin));
    let p = out.trim().lines().last().unwrap_or("").trim().to_string();
    if p.starts_with('/') && Path::new(&p).exists() {
        Some(p)
    } else {
        None
    }
}

fn this_host() -> String {
    let h = sh("scutil --get LocalHostName").trim().to_string();
    if h.is_empty() {
        sh("hostname -s").trim().to_string()
    } else {
        h
    }
}

fn free_gb(path: &str) -> Option<u64> {
    let out = sh(&format!("df -Pk {} 2>/dev/null", shell_quote(path)));
    let line = out.lines().nth(1)?;
    let avail_kb: u64 = line.split_whitespace().nth(3)?.parse().ok()?;
    Some(avail_kb / 1024 / 1024)
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

// (order, service name, bsd device) for every configured network service
fn network_services() -> Vec<(usize, String, String)> {
    let out = sh("networksetup -listnetworkserviceorder 2>/dev/null");
    let mut v: Vec<(usize, String, String)> = Vec::new();
    let mut order = 0usize;
    for line in out.lines() {
        let t = line.trim();
        if t.starts_with("(Hardware Port:") {
            if let Some(last) = v.last_mut() {
                if let Some(d) = t.split("Device: ").nth(1) {
                    last.2 = d.trim_end_matches(')').trim().to_string();
                }
            }
        } else if t.starts_with('(') && t.contains(')') {
            let inner = &t[1..t.find(')').unwrap_or(1)];
            // "(1) Wi-Fi" = enabled, "(*) Wi-Fi" = disabled — both hold a slot
            if inner.parse::<usize>().is_ok() || inner == "*" {
                let name = t[t.find(')').unwrap_or(0) + 1..].trim().to_string();
                if !name.is_empty() {
                    order += 1;
                    v.push((order, name, String::new()));
                }
            }
        }
    }
    v
}

fn iface_active(dev: &str) -> bool {
    if dev.is_empty() {
        return false;
    }
    sh(&format!("ifconfig {} 2>/dev/null", safe_host(dev))).contains("status: active")
}

fn is_ethernet(name: &str) -> bool {
    let n = name.to_lowercase();
    (n.contains("ethernet") || n.contains("lan")) && !n.contains("bridge")
}

fn is_wifi(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("wi-fi") || n.contains("wifi") || n.contains("airport")
}

// ---------------------------------------------------------------------------
// Setup & Verify
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
struct Check {
    id: String,
    stage: u8,
    stage_label: String,
    label: String,
    status: String, // ok | warn | fail
    detail: String,
    fix: String,
    action: String,       // "" = no button
    action_label: String, // button text
}

impl Check {
    fn new(id: &str, stage: u8, stage_label: &str, label: &str) -> Self {
        Self {
            id: id.into(),
            stage,
            stage_label: stage_label.into(),
            label: label.into(),
            status: "fail".into(),
            detail: String::new(),
            fix: String::new(),
            action: String::new(),
            action_label: String::new(),
        }
    }
    fn ok(mut self, detail: impl Into<String>) -> Self {
        self.status = "ok".into();
        self.detail = detail.into();
        self
    }
    fn warn(mut self, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        self.status = "warn".into();
        self.detail = detail.into();
        self.fix = fix.into();
        self
    }
    fn fail(mut self, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        self.status = "fail".into();
        self.detail = detail.into();
        self.fix = fix.into();
        self
    }
    fn button(mut self, action: &str, label: &str) -> Self {
        self.action = action.into();
        self.action_label = label.into();
        self
    }
}

#[derive(Serialize, Clone)]
struct WorkerInfo {
    host: String,
    state: String, // rendering | idle
    job: String,
    age_secs: u64,
}

#[derive(Serialize)]
struct VerifyReport {
    host: String,
    root: String,
    is_coordinator: bool,
    checks: Vec<Check>,
    workers: Vec<WorkerInfo>,
    ok: usize,
    warn: usize,
    fail: usize,
    ready: bool,
}

const S0: &str = "0 · Get the files";
const S1: &str = "1 · Coordinator: the shared queue";
const S2: &str = "2 · Wire the network";
const S3: &str = "3 · Models on the share";
const S4: &str = "4 · This Mac: install + join";
const S5: &str = "5 · App connection";

// Look for the farm scripts if the user hasn't told us where they are.
fn detect_repo(cfg: &Config) -> Option<String> {
    let explicit = cfg.repo_dir.trim();
    if !explicit.is_empty() {
        return Path::new(&format!("{}/farm_worker.sh", explicit))
            .exists()
            .then(|| explicit.to_string());
    }
    let h = home();
    let bases = [
        h.clone(),
        format!("{}/Desktop", h),
        format!("{}/Downloads", h),
        format!("{}/Documents", h),
        format!("{}/video-gen", h),
    ];
    for b in bases {
        if Path::new(&format!("{}/farm_worker.sh", b)).exists() {
            return Some(b);
        }
        if let Ok(rd) = std::fs::read_dir(&b) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() && p.join("farm_worker.sh").exists() {
                    return Some(p.to_string_lossy().to_string());
                }
            }
        }
    }
    None
}

fn mtime_age(p: &Path) -> u64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .map(|d| d.as_secs())
        .unwrap_or(u64::MAX)
}

fn check_files(cfg: &Config, checks: &mut Vec<Check>) {
    let c = Check::new("repo", 0, S0, "Farm scripts downloaded to this Mac");
    checks.push(match detect_repo(cfg) {
        Some(dir) => c
            .ok(format!("found at {}", dir))
            .button("open_repo", "Reveal"),
        None => c
            .fail(
                "farm_worker.sh not found in Home / Desktop / Downloads / Documents",
                "Download the repo (green Code → Download ZIP on GitHub), unzip it, then set its \
                 folder in Settings below — or just drop it on your Desktop.",
            )
            .button("open_github", "Open GitHub"),
    });
}

fn check_coordinator(cfg: &Config, host: &str, is_coord: bool, checks: &mut Vec<Check>) {
    // Which Mac hosts the share?
    let c = Check::new("coordinator_set", 1, S1, "Coordinator Mac named");
    checks.push(if cfg.coordinator.trim().is_empty() {
        c.fail(
            "not set",
            "In Settings below, enter the coordinator Mac's name (System Settings → General → \
             About → Name on that Mac).",
        )
    } else if is_coord {
        c.ok(format!("this Mac ({}) is the coordinator", host))
    } else {
        c.ok(format!("{}.local", safe_host(&cfg.coordinator)))
    });

    if is_coord {
        // Stage 1 only applies on the coordinator itself.
        let listening = sh("netstat -an -p tcp 2>/dev/null | grep -c '\\.445 .*LISTEN'")
            .trim()
            .parse::<u32>()
            .unwrap_or(0)
            > 0;
        let c = Check::new("file_sharing", 1, S1, "File Sharing (SMB) turned on");
        checks.push(if listening {
            c.ok("port 445 is listening")
        } else {
            c.fail(
                "SMB isn't listening on this Mac",
                "System Settings → General → Sharing → turn File Sharing ON, then ⓘ → + → add \
                 your RenderFarm folder.",
            )
            .button("open_sharing", "Open Sharing settings")
        });

        let local = format!("{}/{}", home(), cfg.share_name.trim());
        let c = Check::new("share_folder", 1, S1, "RenderFarm folder exists");
        checks.push(if Path::new(&local).is_dir() {
            c.ok(local.clone())
        } else {
            c.fail(
                format!("{} not found", local),
                "Create it, then add it under Shared Folders in Sharing settings.",
            )
            .button("create_share_folder", "Create it")
        });
    } else {
        let name = safe_host(&cfg.coordinator);
        let c = Check::new("coordinator_ping", 1, S1, "Coordinator reachable on the network");
        checks.push(if name.is_empty() {
            c.warn("skipped — no coordinator name set", "Set it in Settings below.")
        } else {
            let out = sh(&format!("ping -c1 -t2 {}.local 2>&1", name));
            if out.contains("bytes from") {
                let ms = out
                    .split("time=")
                    .nth(1)
                    .and_then(|s| s.split_whitespace().next())
                    .unwrap_or("?")
                    .to_string();
                c.ok(format!("{}.local replies in {} ms", name, ms))
            } else {
                c.fail(
                    format!("no reply from {}.local", name),
                    "Check the name is exactly the coordinator's Mac name, that it's awake, and \
                     that both Macs are cabled into the same switch (or on the same WiFi).",
                )
            }
        });
    }
}

fn check_network(checks: &mut Vec<Check>) {
    let services = network_services();
    let wifi = services.iter().find(|(_, n, _)| is_wifi(n)).cloned();
    let eth: Vec<_> = services
        .iter()
        .filter(|(_, n, _)| is_ethernet(n))
        .cloned()
        .collect();
    let eth_live = eth.iter().find(|(_, _, d)| iface_active(d)).cloned();

    // 2a — ethernet plugged into the switch
    let c = Check::new("ethernet", 2, S2, "Ethernet cable into the gigabit switch");
    checks.push(match (&eth_live, eth.is_empty()) {
        (Some((_, name, dev)), _) => {
            let media = sh(&format!("networksetup -getMedia {} 2>/dev/null", shell_quote(name)));
            let gig = media.contains("1000base") || media.contains("2500base") || media.contains("10Gbase");
            if gig {
                c.ok(format!("{} ({}) up at gigabit", name, dev))
            } else {
                let speed = media
                    .lines()
                    .find(|l| l.starts_with("Current:"))
                    .unwrap_or("Current: unknown")
                    .replace("Current: ", "");
                c.warn(
                    format!("{} ({}) is up but linked at {}", name, dev, speed.trim()),
                    "That's a 10/100 link — big file moves will crawl. Most cheap USB-C adapters \
                     are secretly 10/100; use one whose box says Gigabit / 1000 Mbps, and check \
                     the switch port + cable (Cat5e or better).",
                )
            }
        }
        (None, true) => c.fail(
            "no ethernet service on this Mac",
            "Plug in a Gigabit USB-C → Ethernet adapter (MacBooks), then cable it to the switch.",
        )
        .button("open_network", "Open Network settings"),
        (None, false) => c.fail(
            "ethernet exists but the link is down",
            "Plug the cable into the switch (and check the adapter is seated). The switch does \
             not need internet — an isolated island still resolves .local names.",
        )
        .button("open_network", "Open Network settings"),
    });

    // 2b — Wi-Fi above Ethernet in Service Order
    let c = Check::new("service_order", 2, S2, "Wi-Fi ABOVE Ethernet in Service Order");
    checks.push(match (&wifi, eth.first()) {
        (Some((wi, _, _)), Some((ei, en, _))) => {
            if wi < ei {
                c.ok("internet over WiFi, farm traffic over the switch")
            } else {
                c.fail(
                    format!("“{}” is above Wi-Fi", en),
                    "System Settings → Network → ⋯ → Set Service Order → drag Wi-Fi ABOVE \
                     Ethernet. Otherwise macOS tries to reach the internet through the switch.",
                )
                .button("open_network", "Open Network settings")
            }
        }
        (Some(_), None) => c.warn(
            "no ethernet service to order against",
            "Add the ethernet adapter first — then put Wi-Fi above it.",
        ),
        _ => c.warn(
            "no Wi-Fi service found",
            "The Mac needs WiFi for model downloads during setup.",
        ),
    });
}

fn check_share(cfg: &Config, root: &str, is_coord: bool, checks: &mut Vec<Check>) {
    let rootp = Path::new(root);

    // 4a — mounted
    let mounts = sh("/sbin/mount 2>/dev/null");
    let mount_line = mounts
        .lines()
        .find(|l| l.contains(&format!(" on {} (", root)))
        .unwrap_or("")
        .to_string();
    let is_smb = mount_line.contains("smbfs");

    let c = Check::new("share_mounted", 4, S4, "Shared queue folder mounted");
    checks.push(if !rootp.is_dir() {
        c.fail(
            format!("{} doesn't exist", root),
            format!(
                "Finder → Go → Connect to Server → {} → Connect. It mounts at {}.",
                cfg.share_url(),
                root
            ),
        )
        .button("mount_share", "Mount share")
    } else if is_smb {
        c.ok(format!("{} (SMB)", root))
            .button("open_farm", "Reveal")
    } else if is_coord && mount_line.is_empty() {
        // On the coordinator the queue can legitimately be a local folder.
        c.ok(format!("{} (local folder on the coordinator)", root))
            .button("open_farm", "Reveal")
    } else {
        c.warn(
            format!("{} exists but isn't an SMB mount", root),
            "That's a stray local folder with the same name — jobs written there never reach the \
             other Macs. Move/rename it, then mount the real share.",
        )
        .button("mount_share", "Mount share")
    });

    if !rootp.is_dir() {
        return;
    }

    // 4b — the queue dirs exist AND are writable over SMB
    let dirs = ["queue", "queue/hi", "running", "done", "failed", "assets"];
    let mut missing: Vec<&str> = Vec::new();
    let mut unwritable: Vec<&str> = Vec::new();
    for d in dirs {
        let p = rootp.join(d);
        if !p.is_dir() {
            missing.push(d);
            continue;
        }
        let probe = p.join(".probe");
        match std::fs::write(&probe, b"ok") {
            Ok(_) => {
                let _ = std::fs::remove_file(&probe);
            }
            Err(_) => unwritable.push(d),
        }
    }
    let c = Check::new("queue_dirs", 4, S4, "Queue folders present and writable");
    checks.push(if !unwritable.is_empty() {
        c.fail(
            format!("can't write to: {}", unwritable.join(", ")),
            "You're connected to the share as a user without write access. Re-mount it signed in \
             as a user that can write (Finder → Connect to Server → Connect As…).",
        )
        .button("mount_share", "Re-mount share")
    } else if !missing.is_empty() {
        c.warn(
            format!("missing: {}", missing.join(", ")),
            "The worker creates these on first run — or create them now.",
        )
        .button("create_dirs", "Create folders")
    } else {
        c.ok("queue, queue/hi, running, done, failed, assets — all writable")
    });

    // 3 — models staged on the share (coordinator's job, everyone can see it)
    let models = list_dir(&rootp.join("models")).len();
    let loras = list_dir(&rootp.join("loras")).len();
    let c = Check::new("models_staged", 3, S3, "Models staged on the share");
    checks.push(if models > 0 {
        c.ok(format!("{} model(s), {} LoRA(s) on the share", models, loras))
    } else {
        c.fail(
            "share/models is empty",
            "On the COORDINATOR only, run:  FARM_ROOT=<share> ./seed_farm_assets.sh — it copies \
             everything in MANIFEST.txt onto the share so workers pull it over the switch instead \
             of HuggingFace.",
        )
    });
}

fn check_this_mac(cfg: &Config, checks: &mut Vec<Check>) {
    // toolchain
    let ltx = cfg.ltx();
    let ltx_bin = format!("{}/.venv/bin/ltx-2-mlx", ltx);
    let c = Check::new("toolchain", 4, S4, "LTX2-MLX runtime installed");
    checks.push(if Path::new(&ltx_bin).exists() {
        c.ok(ltx_bin.clone())
    } else {
        c.fail(
            format!("not built at {}", ltx),
            "Double-click setup.command in the farm folder (installs Homebrew, uv, LTX2-MLX, \
             mflux — 15–30 min, mostly unattended). If it's installed somewhere else, set LTX dir \
             in Settings below.",
        )
        .button("open_repo", "Reveal farm folder")
    });

    let c = Check::new("mflux", 4, S4, "mflux (test proofs + LoRA stills)");
    checks.push(match which_login("mflux-generate-z-image-turbo") {
        Some(p) => c.ok(p),
        None => c.warn(
            "not on PATH",
            "Only needed for --test proof stills and LoRA jobs. setup.command installs it \
             (uv tool install mflux).",
        ),
    });

    // models pulled locally by provision.command
    let hub = format!("{}/.cache/huggingface/hub", home());
    let local_models = list_dir(Path::new(&hub))
        .iter()
        .filter(|n| n.starts_with("models--"))
        .count();
    let c = Check::new("models_local", 4, S4, "Models pulled onto this Mac");
    checks.push(if local_models > 0 {
        c.ok(format!("{} model(s) in the local HuggingFace cache", local_models))
    } else {
        c.fail(
            "no models in ~/.cache/huggingface/hub",
            "Double-click provision.command in the farm folder — it rsyncs the models + LoRAs off \
             the share over the switch (minutes, not days).",
        )
        .button("open_repo", "Reveal farm folder")
    });

    // disk headroom (the worker's own guard)
    let c = Check::new("disk", 4, S4, "Free disk for models + renders");
    checks.push(match free_gb(&home()) {
        Some(gb) if gb >= cfg.min_free_gb.max(60) => c.ok(format!("{} GB free", gb)),
        Some(gb) if gb >= cfg.min_free_gb => c.warn(
            format!("{} GB free", gb),
            format!(
                "Above the worker's {} GB floor, but the models alone are ~60 GB. Clear space \
                 before provisioning.",
                cfg.min_free_gb
            ),
        ),
        Some(gb) => c.fail(
            format!("{} GB free — below the {} GB floor", gb, cfg.min_free_gb),
            "The worker pauses instead of rendering. Free up space (or lower MIN_FREE_GB in \
             Settings if you know what you're doing).",
        ),
        None => c.warn("couldn't read free space", ""),
    });

    // is this Mac actually in the farm right now?
    let running = sh("pgrep -fl farm_worker.sh 2>/dev/null");
    let is_running = running
        .lines()
        .any(|l| l.contains("farm_worker.sh") && !l.contains("pgrep"));
    let c = Check::new("worker_running", 4, S4, "This Mac is running a worker");
    checks.push(if is_running {
        c.ok(format!("farm_worker.sh is live (profile {})", cfg.perf))
    } else {
        c.warn(
            "not running",
            "Double-click start_worker.command to join the farm. Leave that window open — \
             closing it stops this worker. (Skip if this Mac only monitors.)",
        )
        .button("start_worker", "Start worker")
    });
}

fn read_workers(root: &str) -> Vec<WorkerInfo> {
    let running = Path::new(root).join("running");
    let names = list_dir_all(&running);
    let mut map: HashMap<String, WorkerInfo> = HashMap::new();

    // registered: running/.worker.<HOST>.lock (written when start_worker launches)
    for n in names.iter().filter(|n| n.starts_with(".worker.") && n.ends_with(".lock")) {
        let host = n
            .trim_start_matches(".worker.")
            .trim_end_matches(".lock")
            .to_string();
        map.insert(
            host.clone(),
            WorkerInfo {
                host,
                state: "idle".into(),
                job: String::new(),
                age_secs: mtime_age(&running.join(n)),
            },
        );
    }
    // rendering right now: a fresh <jobfile>.heartbeat (touched every 30s)
    for n in names.iter().filter(|n| n.ends_with(".heartbeat")) {
        let age = mtime_age(&running.join(n));
        if age > 120 {
            continue; // stale — that worker stopped; --reap will requeue it
        }
        let job_name = n.trim_end_matches(".heartbeat");
        let host = parse_host(job_name);
        map.insert(
            host.clone(),
            WorkerInfo {
                host,
                state: "rendering".into(),
                job: parse_id(job_name),
                age_secs: age,
            },
        );
    }
    let mut v: Vec<_> = map.into_values().collect();
    v.sort_by(|a, b| a.host.cmp(&b.host));
    v
}

#[tauri::command]
fn verify_link(cfg_state: State<CfgState>) -> VerifyReport {
    let cfg = cfg_state.0.lock().unwrap().clone();
    let root = cfg.root();
    let host = this_host();
    let is_coord = !cfg.coordinator.trim().is_empty()
        && safe_host(&cfg.coordinator).eq_ignore_ascii_case(&host);

    let mut checks: Vec<Check> = Vec::new();
    check_files(&cfg, &mut checks);
    check_coordinator(&cfg, &host, is_coord, &mut checks);
    check_network(&mut checks);
    check_share(&cfg, &root, is_coord, &mut checks);
    check_this_mac(&cfg, &mut checks);

    let workers = read_workers(&root);
    let c = Check::new("workers", 5, S5, "Other Macs seen on the farm");
    checks.push(if workers.is_empty() {
        c.warn(
            "no workers registered right now",
            "Nobody has start_worker.command running (or the share isn't reachable). Jobs will \
             sit in the queue until a Mac joins.",
        )
    } else {
        let rendering = workers.iter().filter(|w| w.state == "rendering").count();
        c.ok(format!(
            "{} worker(s): {} — {} rendering now",
            workers.len(),
            workers
                .iter()
                .map(|w| w.host.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            rendering
        ))
    });

    // the app's own link to the queue
    let c = Check::new("app_watching", 5, S5, "This app is watching the queue");
    checks.push(if Path::new(&root).join("queue").is_dir() {
        c.ok(format!("polling {} every 2s", root))
    } else {
        c.fail(
            "no queue folder to watch",
            "Fix the share checks above — the tray counts and ping sounds stay silent until the \
             queue is reachable.",
        )
    });

    // stable display order: by stage, keeping insertion order inside a stage
    checks.sort_by_key(|c| c.stage);

    let ok = checks.iter().filter(|c| c.status == "ok").count();
    let warn = checks.iter().filter(|c| c.status == "warn").count();
    let fail = checks.iter().filter(|c| c.status == "fail").count();

    VerifyReport {
        host,
        root,
        is_coordinator: is_coord,
        checks,
        workers,
        ok,
        warn,
        fail,
        ready: fail == 0,
    }
}

// ---------------------------------------------------------------------------
// Config + action commands
// ---------------------------------------------------------------------------

#[tauri::command]
fn get_config(cfg_state: State<CfgState>) -> serde_json::Value {
    let cfg = cfg_state.0.lock().unwrap().clone();
    serde_json::json!({
        "config": cfg,
        "resolved": { "root": cfg.root(), "ltx_dir": cfg.ltx(), "share_url": cfg.share_url() },
        "host": this_host(),
        "config_file": config_path().to_string_lossy(),
    })
}

#[tauri::command]
fn save_config(cfg: Config, cfg_state: State<CfgState>) -> Result<serde_json::Value, String> {
    write_config(&cfg)?;
    {
        let mut cur = cfg_state.0.lock().unwrap();
        *cur = cfg.clone();
    }
    // the watcher re-reads the root each loop, so this hot-reloads within ~2s
    Ok(serde_json::json!({
        "config": cfg,
        "resolved": { "root": cfg.root(), "ltx_dir": cfg.ltx(), "share_url": cfg.share_url() },
    }))
}

#[tauri::command]
fn mount_share(cfg_state: State<CfgState>) -> Result<String, String> {
    let cfg = cfg_state.0.lock().unwrap().clone();
    if cfg.coordinator.trim().is_empty() {
        return Err("Set the coordinator Mac's name first.".into());
    }
    let url = cfg.share_url();
    Command::new("open").arg(&url).spawn().map_err(|e| e.to_string())?;
    Ok(format!("Opening {} — approve it in Finder if it asks.", url))
}

// One place for every "do the thing" button the checklist offers.
#[tauri::command]
fn run_action(action: String, cfg_state: State<CfgState>) -> Result<String, String> {
    let cfg = cfg_state.0.lock().unwrap().clone();
    let root = cfg.root();
    match action.as_str() {
        "open_network" => {
            let _ = Command::new("open")
                .arg("x-apple.systempreferences:com.apple.Network-Settings.extension")
                .spawn();
            Ok("Opened Network settings — use ⋯ → Set Service Order.".into())
        }
        "open_sharing" => {
            let _ = Command::new("open")
                .arg("x-apple.systempreferences:com.apple.Sharing-Settings.extension")
                .spawn();
            Ok("Opened Sharing settings — turn File Sharing on.".into())
        }
        "open_farm" => {
            let _ = Command::new("open").arg(&root).spawn();
            Ok(format!("Opened {}", root))
        }
        "open_repo" => match detect_repo(&cfg) {
            Some(d) => {
                let _ = Command::new("open").arg(&d).spawn();
                Ok(format!("Opened {}", d))
            }
            None => Err("Farm folder not found — set it in Settings.".into()),
        },
        "open_github" => {
            let _ = Command::new("open")
                .arg("https://github.com/aidenwood/VideoGen-NetworkSwitchLoadBalancer")
                .spawn();
            Ok("Opened the repo in your browser.".into())
        }
        "create_share_folder" => {
            let p = cfg.local_root();
            std::fs::create_dir_all(&p).map_err(|e| format!("Couldn't create {} — {}", p, e))?;
            let _ = Command::new("open").arg(&p).spawn();
            Ok(format!("Created {} — now add it in Sharing settings.", p))
        }
        "create_dirs" => {
            // Refuse to even try under /Volumes on a coordinator — that's the
            // mount point workers use, it's root-owned, and the resulting
            // "Permission denied (os error 13)" tells the user nothing.
            if cfg.role == "coordinator" && root.starts_with("/Volumes/") {
                return Err(format!(
                    "This Mac is the coordinator, so the farm folder is {} — not {}. \
                     /Volumes is macOS's mount point for OTHER Macs' shares and can't be \
                     written to. Fixed automatically: press Re-check.",
                    cfg.local_root(),
                    root
                ));
            }
            for d in ["queue", "queue/hi", "running", "done", "failed", "assets", "logs"] {
                let p = Path::new(&root).join(d);
                std::fs::create_dir_all(&p).map_err(|e| {
                    format!(
                        "Couldn't create {} — {}.{}",
                        p.display(),
                        e,
                        if e.kind() == std::io::ErrorKind::PermissionDenied {
                            format!(
                                " Nothing here is writable by you. The coordinator's folder \
                                 should be {}; a worker's should be a mounted share.",
                                cfg.local_root()
                            )
                        } else {
                            String::new()
                        }
                    )
                })?;
            }
            Ok(format!("Queue folders created in {}", root))
        }
        "start_worker" => match detect_repo(&cfg) {
            Some(d) => {
                let script = format!("{}/start_worker.command", d);
                if !Path::new(&script).exists() {
                    return Err(format!("start_worker.command not found in {}", d));
                }
                // open in Terminal so the worker's log stays visible (and closable)
                Command::new("open")
                    .arg("-a")
                    .arg("Terminal")
                    .arg(&script)
                    .spawn()
                    .map_err(|e| e.to_string())?;
                Ok("Launched start_worker.command in Terminal.".into())
            }
            None => Err("Farm folder not found — set it in Settings.".into()),
        },
        // The two long-running installers. Both are idempotent and both print a
        // lot, so they go to Terminal rather than being swallowed by the app.
        "run_setup" => open_script_in_terminal(&cfg, "setup.command"),
        "run_provision" => open_script_in_terminal(&cfg, "provision.command"),
        other => Err(format!("unknown action: {}", other)),
    }
}

fn open_script_in_terminal(cfg: &Config, name: &str) -> Result<String, String> {
    let dir = detect_repo(cfg).ok_or("Farm folder not found — set it in Settings.")?;
    let script = format!("{}/{}", dir, name);
    if !Path::new(&script).exists() {
        return Err(format!("{} not found in {}", name, dir));
    }
    Command::new("open")
        .arg("-a")
        .arg("Terminal")
        .arg(&script)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(format!("Running {} in Terminal — watch that window.", name))
}

// ---------------------------------------------------------------------------
// Guided setup
// ---------------------------------------------------------------------------

// Find Macs already sharing over SMB, so a worker can PICK its coordinator
// instead of being asked to type a hostname it has no way of knowing.
// dns-sd browses forever by design, so run it briefly and take what it found.
fn discover_smb_hosts() -> Vec<String> {
    use std::io::Read;
    use std::process::Stdio;

    let mut child = match Command::new("/usr/bin/dns-sd")
        .args(["-B", "_smb._tcp", "local."])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    std::thread::sleep(Duration::from_millis(2500));
    let _ = child.kill();

    let mut out = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut out);
    }
    let _ = child.wait();

    let me = this_host().to_lowercase();
    let mut seen = HashSet::new();
    let mut hosts = Vec::new();
    for line in out.lines() {
        // ts  Add  flags  if  domain  _smb._tcp.  <instance name>
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 7 || f[1] != "Add" {
            continue;
        }
        let name = f[6..].join(" ");
        if name.is_empty() || name.to_lowercase() == me {
            continue; // don't offer this Mac as its own coordinator
        }
        if seen.insert(name.clone()) {
            hosts.push(name);
        }
    }
    hosts
}

#[tauri::command]
fn discover_coordinators() -> Vec<String> {
    discover_smb_hosts()
}

#[derive(Serialize)]
struct SetupStep {
    id: String,
    title: String,
    body: String,
    done: bool,
    detail: String,
    action: String,
    action_label: String,
    manual: bool, // true = we can only open the pane; the human does the clicking
}

// The wizard's model of "where am I up to". Deliberately recomputed from the
// real world on every call — never from a stored step number — so quitting
// halfway, or doing a step by hand in Finder, both just work.
#[tauri::command]
fn setup_steps(cfg_state: State<CfgState>) -> serde_json::Value {
    let cfg = cfg_state.0.lock().unwrap().clone();
    let root = cfg.root();
    let host = this_host();
    let coord = cfg.role == "coordinator";
    let mut steps: Vec<SetupStep> = Vec::new();

    let push = |v: &mut Vec<SetupStep>,
                id: &str,
                title: &str,
                body: &str,
                done: bool,
                detail: String,
                action: &str,
                action_label: &str,
                manual: bool| {
        v.push(SetupStep {
            id: id.into(),
            title: title.into(),
            body: body.into(),
            done,
            detail,
            action: action.into(),
            action_label: action_label.into(),
            manual,
        });
    };

    if coord {
        let folder = cfg.local_root();
        let has_folder = Path::new(&folder).is_dir();
        push(&mut steps, "folder", "Create the shared folder",
            "Every Mac reads jobs from one folder on this Mac. Make it first.",
            has_folder,
            if has_folder { format!("{} exists", folder) } else { format!("Will create {}", folder) },
            "create_share_folder", "Create the folder", false);

        // Advertised over SMB = File Sharing is on AND this folder is shared.
        let shared = sh("sharing -l 2>/dev/null").to_lowercase()
            .contains(&cfg.share_name.trim().to_lowercase())
            || discover_smb_hosts().iter().any(|h| h.eq_ignore_ascii_case(&host));
        push(&mut steps, "sharing", "Turn on File Sharing",
            "System Settings → General → Sharing → File Sharing ON, then ⓘ → + → add the folder you just made.",
            shared,
            if shared { "This Mac is sharing over SMB".into() }
                 else { "Not advertising a share yet".into() },
            "open_sharing", "Open Sharing settings", true);

        let has_dirs = Path::new(&root).join("queue").is_dir();
        push(&mut steps, "dirs", "Create the queue folders",
            "queue/, running/, done/, failed/, assets/ — the farm's inbox and outbox.",
            has_dirs,
            if has_dirs { format!("{}/queue ready", root) } else { format!("Will create them under {}", root) },
            "create_dirs", "Create them", false);
    } else {
        let picked = !cfg.coordinator.trim().is_empty();
        push(&mut steps, "pick", "Choose the coordinator Mac",
            "The Mac holding the shared folder. Pick it from the list — no typing.",
            picked,
            if picked { format!("Coordinator: {}", cfg.coordinator) } else { "Nothing chosen yet".into() },
            "", "", false);

        let mounted = Path::new(&root).is_dir();
        push(&mut steps, "mount", "Connect to the shared folder",
            "Mounts the coordinator's folder on this Mac. Approve it in Finder if macOS asks.",
            mounted,
            if mounted { format!("Mounted at {}", root) } else { format!("Not mounted — {}", cfg.share_url()) },
            "mount_share", "Connect", false);
    }

    // Both roles need the toolchain and the models.
    let ltx = cfg.ltx();
    let has_ltx = Path::new(&ltx).join(".venv/bin/ltx-2-mlx").exists();
    push(&mut steps, "toolchain", "Install the render toolchain",
        "Homebrew, uv, LTX2-MLX and mflux. 15–30 min, mostly unattended. Safe to re-run.",
        has_ltx,
        if has_ltx { format!("ltx-2-mlx built at {}", ltx) } else { "Not installed on this Mac yet".into() },
        "run_setup", "Run setup", false);

    let models = Path::new(&format!("{}/.cache/huggingface/hub", home())).is_dir()
        && !list_dir_all(Path::new(&format!("{}/.cache/huggingface/hub", home())))
            .iter().filter(|n| n.starts_with("models--")).collect::<Vec<_>>().is_empty();
    push(&mut steps, "models", "Copy the models to this Mac",
        "~60GB pulled off the share over the switch — far faster than HuggingFace.",
        models,
        if models { "Models present in the local HuggingFace cache".into() }
             else { "No models cached locally yet".into() },
        "run_provision", "Provision", false);

    let done = steps.iter().all(|s| s.done);
    serde_json::json!({
        "host": host,
        "role": cfg.role,
        "root": root,
        "share_url": cfg.share_url(),
        "steps": steps,
        "all_done": done,
        "wizard_done": cfg.wizard_done,
    })
}

// Pick coordinator-vs-worker. Sets the sensible default profile at the same
// time: a coordinator is usually someone's actual Mac, so don't hand it 'full'.
#[tauri::command]
fn set_role(role: String, cfg_state: State<CfgState>) -> Result<(), String> {
    let mut guard = cfg_state.0.lock().unwrap();
    guard.role = role;
    // Repoint the farm folder at whatever the chosen role actually means, and
    // heal a share_path left pointing at /Volumes — a coordinator can't write
    // there, and that mismatch is what produced "Permission denied (os error 13)".
    if guard.role == "coordinator" {
        if guard.share_path.trim().is_empty() || guard.share_path.trim().starts_with("/Volumes/") {
            guard.share_path = guard.local_root();
        }
        guard.coordinator = this_host(); // it IS the coordinator
    } else if guard.role == "worker" && guard.share_path.trim() == guard.local_root() {
        guard.share_path = String::new(); // fall back to the /Volumes mount
    }
    write_config(&guard)
}

#[tauri::command]
fn set_coordinator(name: String, cfg_state: State<CfgState>) -> Result<(), String> {
    let mut guard = cfg_state.0.lock().unwrap();
    guard.coordinator = name;
    if guard.share_path.trim().is_empty() {
        guard.share_path = format!("/Volumes/{}", guard.share_name.trim());
    }
    write_config(&guard)
}

#[tauri::command]
fn finish_wizard(cfg_state: State<CfgState>) -> Result<(), String> {
    let mut guard = cfg_state.0.lock().unwrap();
    guard.wizard_done = true;
    write_config(&guard)
}

// ---------------------------------------------------------------------------
// Watcher
// ---------------------------------------------------------------------------

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
        let dirs = ["queue", "running", "done", "failed"];
        let mut seen: HashMap<&str, HashSet<String>> = HashMap::new();
        let mut first = true;
        let mut cur_root = String::new();

        loop {
            // re-read the configured root every tick so Settings hot-reloads
            let root = {
                let cs: State<CfgState> = app.state();
                let r = cs.0.lock().unwrap().root();
                r
            };
            if root != cur_root {
                cur_root = root.clone();
                seen.clear();
                first = true; // don't spam notifications for a folder we just switched to
                let st: State<SharedState> = app.state();
                st.0.lock().unwrap().root = root.clone();
            }

            let mut counts = Counts::default();
            let mut fresh: Vec<Event> = Vec::new();

            for d in dirs {
                let p = Path::new(&root).join(d);
                let names = list_dir(&p);

                let interesting = |n: &str| match d {
                    "queue" => n.ends_with(".job"),
                    "running" => n.contains(".job.") && !n.ends_with(".heartbeat"),
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

// ---------------------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let cfg = load_config();
    let root = cfg.root();

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .manage(CfgState(Mutex::new(cfg)))
        .manage(SharedState(Mutex::new(Farm {
            root,
            ..Default::default()
        })))
        .invoke_handler(tauri::generate_handler![
            get_state,
            show_dashboard,
            get_config,
            save_config,
            verify_link,
            mount_share,
            run_action,
            setup_steps,
            discover_coordinators,
            set_role,
            set_coordinator,
            finish_wizard
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let show = MenuItem::with_id(app, "show", "Open dashboard", true, None::<&str>)?;
            let setup = MenuItem::with_id(app, "setup", "Setup & Verify…", true, None::<&str>)?;
            let openf = MenuItem::with_id(app, "open_folder", "Reveal farm folder", true, None::<&str>)?;
            let sep = PredefinedMenuItem::separator(app)?;
            let quit = PredefinedMenuItem::quit(app, Some("Quit LTX Mac Farm"))?;
            let menu = Menu::with_items(app, &[&show, &setup, &openf, &sep, &quit])?;

            let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;
            let _tray = TrayIconBuilder::with_id("main")
                .icon(icon)
                .icon_as_template(true)
                .tooltip("LTX Mac Farm")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" | "setup" => {
                        if let Some(w) = app.get_webview_window("dash") {
                            // ask the UI which tab to land on
                            let tab = if event.id().as_ref() == "setup" { "setup" } else { "dash" };
                            let _ = w.eval(&format!("window.__openTab && window.__openTab('{}')", tab));
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "open_folder" => {
                        let root = {
                            let cs: State<CfgState> = app.state();
                            let r = cs.0.lock().unwrap().root();
                            r
                        };
                        let _ = Command::new("open").arg(root).spawn();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(role: &str, share_path: &str) -> Config {
        Config { role: role.into(), share_path: share_path.into(), ..Default::default() }
    }

    // The regression: a coordinator with no share_path fell through to
    // /Volumes/RenderFarm, which is root-owned, so creating the queue folders
    // died with "Permission denied (os error 13)".
    #[test]
    fn coordinator_root_is_local_not_volumes() {
        let c = cfg("coordinator", "");
        assert_eq!(c.root(), format!("{}/RenderFarm", home()));
        assert!(!c.root().starts_with("/Volumes/"));
    }

    #[test]
    fn worker_root_is_the_mount_point() {
        std::env::remove_var("FARM_ROOT");
        assert_eq!(cfg("worker", "").root(), "/Volumes/RenderFarm");
    }

    // An install already broken by the old behaviour must fix itself on load.
    #[test]
    fn normalize_heals_a_coordinator_pointed_at_volumes() {
        let mut c = cfg("coordinator", "/Volumes/RenderFarm");
        c.normalize();
        assert_eq!(c.share_path, format!("{}/RenderFarm", home()));
        assert!(!c.coordinator.is_empty(), "coordinator should name itself");
    }

    #[test]
    fn normalize_leaves_a_worker_alone() {
        let mut c = cfg("worker", "/Volumes/RenderFarm");
        c.normalize();
        assert_eq!(c.share_path, "/Volumes/RenderFarm");
    }

    // share_name must flow through both paths, not be hardcoded.
    #[test]
    fn custom_share_name_is_respected() {
        let mut c = cfg("coordinator", "");
        c.share_name = "Renders2".into();
        assert_eq!(c.root(), format!("{}/Renders2", home()));
        let mut w = cfg("worker", "");
        w.share_name = "Renders2".into();
        std::env::remove_var("FARM_ROOT");
        assert_eq!(w.root(), "/Volumes/Renders2");
    }
}
