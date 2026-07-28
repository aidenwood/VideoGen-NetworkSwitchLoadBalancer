// LTX Mac Farm — render-farm menubar monitor + in-app setup checker.
//
// Four halves (the app grew):
//   1. WATCHER  — polls the shared farm folder (FSEvents is unreliable over SMB,
//      so we poll every 2s and diff), fires a native notification + distinct
//      sound on each ping event, keeps a tray tooltip + dashboard window live.
//      It also publishes this Mac's presence onto the share so everyone else's
//      Team view can see who is connected and what their Mac is doing.
//   2. SETUP    — a live "Setup & Verify" view: every step from the README as a
//      check that reports ✅/⚠️/❌ for THIS Mac, with the exact fix and a button
//      that performs it. New Macs join the farm without reading the README.
//   3. BOARD    — the queue as a kanban board (see jobs.rs): reorder what renders
//      next, open finished clips, queue variants of a shot.
//   4. GATEWAY  — the same UI served over HTTP (see web.rs) so the team can use
//      all of the above from a browser, on any Mac or phone on the office LAN.
//
// ONE COMMAND SURFACE. Everything the UI can do goes through Core::dispatch —
// the Tauri `bridge` command and the gateway's POST /api/invoke both call it, so
// a feature cannot exist in the popover but be missing in the browser.
//
// Events (a "ping" = a job moving through the pipeline):
//   queue/*.job  new  -> 📤 sent      (a job was dispatched)        sound: Tink
//   running/*    new  -> 📥 received  (a Mac picked it up)          sound: Ping
//   done/*.ok    new  -> ✅ done      (a Mac finished a render)     sound: Glass
//   failed/*     new  -> ❌ failed                                   sound: Basso

mod jobs;
mod web;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
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
    // --- who's at this Mac (Team view) ---
    member: String,       // the person sitting at it; defaults to the macOS full name
    // --- web gateway (see web.rs) ---
    web_enabled: bool,    // serve the UI over HTTP at all
    web_port: u16,        // preferred port; the gateway hunts upward if it's taken
    web_lan: bool,        // false = 127.0.0.1 only, true = reachable from the LAN
    web_token: String,    // the key LAN clients must present; generated once
    web_open_on_launch: bool, // open the browser view when the app starts
    // --- overnight autopilot (see jobs::autopilot_tick) ---
    autopilot: bool,      // this Mac babysits the farm unattended
    autopilot_retry: u32, // re-run a failed job this many times
    stale_min: u64,       // requeue an in-flight job whose worker went quiet
    fail_streak: u32,     // pause the queue after this many failures in a row
    presets: Vec<serde_json::Value>, // saved composer setups
}

pub(crate) fn home() -> String {
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
            member: String::new(),
            web_enabled: true,
            web_port: 8787,
            web_lan: false,
            web_token: String::new(),
            web_open_on_launch: true,
            autopilot: false,
            autopilot_retry: 1,
            stale_min: 20,
            fail_streak: 5,
            presets: Vec::new(),
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
    // Tests redirect this so they can never touch a real install's config; it's
    // also a handy escape hatch for a second, throwaway setup on one Mac.
    if let Ok(dir) = std::env::var("FARM_CONFIG_DIR") {
        return PathBuf::from(dir).join("config.json");
    }
    PathBuf::from(format!(
        "{}/Library/Application Support/design.aidxn.ltx-mac-farm/config.json",
        home()
    ))
}

fn load_config() -> Config {
    let raw = std::fs::read_to_string(config_path()).ok();
    let mut cfg: Config = raw
        .as_deref()
        .and_then(|s| serde_json::from_str::<Config>(s).ok())
        .unwrap_or_default();
    let before = serde_json::to_string(&cfg).unwrap_or_default();
    cfg.normalize();
    // normalize() can MINT things (the gateway key, the member name). Persist
    // them now or every launch would invent a new key and every team link the
    // user had saved would stop working.
    if serde_json::to_string(&cfg).unwrap_or_default() != before {
        let _ = write_config(&cfg);
    }
    cfg
}

impl Config {
    // Repair configs written before root() knew about roles. A coordinator with
    // no share_path used to resolve to /Volumes/<name>, which is root-owned, so
    // creating the queue folders failed with EACCES. Healing on load means an
    // already-broken install fixes itself on next launch — nobody has to redo
    // the wizard or hand-edit JSON.
    fn normalize(&mut self) {
        // The gateway key is generated once, on the first launch that needs one,
        // and then never changes — the team's saved links keep working.
        if self.web_token.trim().len() != 32 {
            self.web_token = web::new_token();
        }
        if self.web_port < 1024 {
            self.web_port = 8787; // below 1024 needs root; nobody wants that here
        }
        if self.member.trim().is_empty() {
            self.member = jobs::default_member_name();
        }
        self.stale_min = self.stale_min.clamp(5, 240);
        self.autopilot_retry = self.autopilot_retry.min(5);
        self.fail_streak = self.fail_streak.clamp(2, 50);
        if self.presets.len() > 40 {
            self.presets.truncate(40);
        }

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
pub(crate) fn safe_host(s: &str) -> String {
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

// Everything the UI can act on lives here, behind one handle, because there are
// now two front doors (the popover and the web gateway) and they must share the
// same config and the same watcher state — not two copies that drift.
pub(crate) struct Core {
    pub cfg: Mutex<Config>,
    pub farm: Mutex<Farm>,
    pub gateway: Mutex<Option<web::Gateway>>,
    // Bumped whenever settings/role/name change. Every surface polls get_state
    // every 2s and reloads itself when this moves, which is what makes the
    // popover and an open browser tab feel like one app instead of two copies:
    // change the farm folder in the popover and the phone follows within 2s.
    pub rev: Mutex<u64>,
    // Estimates come from every sidecar in done/, which is an SMB read per
    // finished clip — far too much to redo on every 3s board poll, and it barely
    // changes. Cached for STATS_TTL seconds.
    pub stats: Mutex<Option<(u64, jobs::Stats)>>,
    // A handle back to the Arc this Core lives in. The gateway needs an owned
    // Arc<Core> to hand its serving threads, and dispatch only ever has &self,
    // so Core keeps a weak pointer to itself rather than resorting to unsafe.
    me: Mutex<std::sync::Weak<Core>>,
}

impl Core {
    fn new_arc(cfg: Config, farm: Farm) -> Arc<Core> {
        let core = Arc::new(Core {
            cfg: Mutex::new(cfg),
            farm: Mutex::new(farm),
            gateway: Mutex::new(None),
            rev: Mutex::new(1),
            stats: Mutex::new(None),
            me: Mutex::new(std::sync::Weak::new()),
        });
        *core.me.lock().unwrap() = Arc::downgrade(&core);
        core
    }

    fn arc(&self) -> Option<Arc<Core>> {
        self.me.lock().ok()?.upgrade()
    }

    // Call after anything a different surface would want to redraw for.
    fn bump(&self) {
        if let Ok(mut r) = self.rev.lock() {
            *r += 1;
        }
    }
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

// same as list_dir but keeps dotfiles (worker locks live at running/.worker.<host>.lock)
pub(crate) fn list_dir_all(p: &Path) -> Vec<String> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            v.push(e.file_name().to_string_lossy().to_string());
        }
    }
    v
}

// "<stamp>__<id>.job[.host.pid...]" -> "<id>"
pub(crate) fn parse_id(name: &str) -> String {
    let after = name.splitn(2, "__").nth(1).unwrap_or(name);
    after.split(".job").next().unwrap_or(after).to_string()
}

// "...job.<HOST>.<pid>[.ok|.rcN]" -> "<HOST>"
pub(crate) fn parse_host(name: &str) -> String {
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

pub(crate) fn sh(cmd: &str) -> String {
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

pub(crate) fn this_host() -> String {
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
        format!("{}/Developer", h),
        format!("{}/video-gen", h),
    ];
    // Search a few levels down, not one. People keep repos in a projects folder
    // (~/Desktop/00 - Aidxn/LTX Mac Farm (…)), which a single-level scan misses
    // entirely — that's a "Farm folder not found" on a Mac where it's right there.
    for b in bases {
        if let Some(found) = find_repo_under(Path::new(&b), 3) {
            return Some(found);
        }
    }
    None
}

// Depth-limited hunt for the folder holding farm_worker.sh. Pruned hard so it
// stays instant: no dot-dirs, no dependency/build trees, no Library.
fn find_repo_under(dir: &Path, depth: u8) -> Option<String> {
    if dir.join("farm_worker.sh").exists() {
        return Some(dir.to_string_lossy().to_string());
    }
    if depth == 0 {
        return None;
    }
    const SKIP: [&str; 7] = [
        "node_modules", "target", "Library", ".git", "venv", ".venv", "Pictures",
    ];
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| {
            p.file_name()
                .map(|n| {
                    let n = n.to_string_lossy();
                    !n.starts_with('.') && !SKIP.contains(&n.as_ref())
                })
                .unwrap_or(false)
        })
        .collect();
    entries.sort();
    entries
        .into_iter()
        .find_map(|p| find_repo_under(&p, depth - 1))
}

pub(crate) fn mtime_age(p: &Path) -> u64 {
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

fn verify_link(cfg: &Config) -> VerifyReport {
    let cfg = cfg.clone();
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

fn get_config_json(core: &Core) -> serde_json::Value {
    let cfg = core.cfg.lock().unwrap().clone();
    serde_json::json!({
        "config": cfg,
        "resolved": { "root": cfg.root(), "ltx_dir": cfg.ltx(), "share_url": cfg.share_url() },
        "host": this_host(),
        "config_file": config_path().to_string_lossy(),
        "gateway": gateway_json(core),
        "presets": cfg.presets,
    })
}

// MERGE, don't replace. The UI has two separate settings forms (the share and
// the gateway) and each posts only its own fields; a wholesale replace made the
// other form's values — and role/wizard_done — silently fall back to defaults,
// which is how saving a port could reopen the setup wizard.
fn merge_config(current: &Config, incoming: &serde_json::Value) -> Result<Config, String> {
    let mut base = serde_json::to_value(current).map_err(|e| e.to_string())?;
    let (Some(base_map), Some(patch)) = (base.as_object_mut(), incoming.as_object()) else {
        return Err("config must be an object".into());
    };
    for (k, v) in patch {
        if !v.is_null() {
            base_map.insert(k.clone(), v.clone());
        }
    }
    serde_json::from_value(base).map_err(|e| format!("bad config: {}", e))
}

fn save_config_core(core: &Core, mut cfg: Config) -> Result<serde_json::Value, String> {
    // The UI never sends the gateway key back (it isn't a setting anyone should
    // be able to blank by accident), so carry the stored one across.
    let (old_port, old_lan, old_enabled) = {
        let cur = core.cfg.lock().unwrap();
        if cfg.web_token.trim().len() != 32 {
            cfg.web_token = cur.web_token.clone();
        }
        (cur.web_port, cur.web_lan, cur.web_enabled)
    };
    cfg.normalize();
    write_config(&cfg)?;
    let restart = cfg.web_port != old_port || cfg.web_lan != old_lan || cfg.web_enabled != old_enabled;
    {
        let mut cur = core.cfg.lock().unwrap();
        *cur = cfg.clone();
    }
    // Port/LAN changes take effect immediately rather than on next launch —
    // being told "restart the app" after ticking a checkbox is the kind of thing
    // that makes people give up on the feature.
    if restart {
        restart_gateway(core);
    }
    core.bump();
    // the watcher re-reads the root each loop, so the rest hot-reloads within ~2s
    Ok(serde_json::json!({
        "config": cfg,
        "resolved": { "root": cfg.root(), "ltx_dir": cfg.ltx(), "share_url": cfg.share_url() },
        "gateway": gateway_json(core),
    }))
}

// Shared so run_action can dispatch it too. The UI used to special-case
// mount_share and call it directly, which meant two ways to invoke one button
// and no single list of valid actions to check against — --selftest flagged it.
fn do_mount(cfg: &Config) -> Result<String, String> {
    if cfg.coordinator.trim().is_empty() {
        return Err("Set the coordinator Mac's name first.".into());
    }
    if cfg.role == "coordinator" {
        return Err(format!(
            "This Mac is the coordinator — it hosts {} locally, there is nothing to mount.",
            cfg.local_root()
        ));
    }
    let url = cfg.share_url();
    Command::new("open").arg(&url).spawn().map_err(|e| e.to_string())?;
    Ok(format!("Opening {} — approve it in Finder if it asks.", url))
}

// One place for every "do the thing" button the checklist offers.
fn run_action(action: &str, cfg: &Config) -> Result<String, String> {
    let cfg = cfg.clone();
    let root = cfg.root();
    match action {
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
        // Through the wrapper like the rest: start_worker.command needs to be
        // told THIS Mac's FARM_ROOT and coordinator, or it falls back to
        // /Volumes/RenderFarm and a placeholder hostname.
        "start_worker" => open_script_in_terminal(&cfg, "start_worker.command"),
        // The two long-running installers. Both are idempotent and both print a
        // lot, so they go to Terminal rather than being swallowed by the app.
        "mount_share" => do_mount(&cfg),
        "run_setup" => open_script_in_terminal(&cfg, "setup.command"),
        "run_provision" => open_script_in_terminal(&cfg, "provision.command"),
        // Coordinator-only: push this Mac's models + LoRAs onto the share so
        // the workers pull them over the switch instead of from HuggingFace.
        "seed_assets" => open_script_in_terminal(&cfg, "seed_farm_assets.sh"),
        other => Err(format!("unknown action: {}", other)),
    }
}

// Every action the UI is allowed to ask for. Kept as data so --selftest can
// prove the wizard never references an action the backend doesn't handle —
// a typo there is a dead button, and dead buttons is exactly how this went
// wrong four times in a row.
const KNOWN_ACTIONS: [&str; 12] = [
    "open_network", "open_sharing", "open_farm", "open_repo", "open_github",
    "create_share_folder", "create_dirs", "start_worker", "run_setup",
    "run_provision", "seed_assets", "mount_share",
];

// The exact shell line the app hands to Terminal. Pure: no side effects, so
// the self-test can assert on it.
//
// Two problems solved here at once.
//   1. `open -a Terminal <script>` starts a fresh login shell and passes NO
//      environment, so the script fell back to FARM_ROOT=/Volumes/RenderFarm.
//      On a coordinator that's the wrong folder entirely.
//   2. `open` on an unsigned, un-notarised .command launched BY AN APP trips
//      Gatekeeper: "Apple could not verify … is free of malware", offering
//      only Move to Bin. A wrapper written to /tmp hit the same wall.
// Telling Terminal to `do script` runs a command STRING through the shell:
// nothing is "opened", Gatekeeper isn't involved, and env prefixes inline.
fn script_command(cfg: &Config, dir: &str, name: &str) -> String {
    let lora_dir = if cfg.lora_dir.trim().is_empty() {
        format!("{}/farm-loras", home())
    } else {
        cfg.lora_dir.trim().to_string()
    };
    format!(
        "cd {dir} && FARM_ROOT={root} LTX_DIR={ltx} LORA_DIR={lora} COORDINATOR={coord} ./{name}",
        dir = shell_quote(dir),
        root = shell_quote(&cfg.root()),
        ltx = shell_quote(&cfg.ltx()),
        lora = shell_quote(&lora_dir),
        coord = shell_quote(&safe_host(&cfg.coordinator)),
        name = name,
    )
}

fn open_script_in_terminal(cfg: &Config, name: &str) -> Result<String, String> {
    let dir = detect_repo(cfg).ok_or_else(|| {
        format!(
            "Can't find the farm scripts (looking for farm_worker.sh under {}/Desktop, \
             Documents, Downloads and Developer). Use \u{201c}Choose folder\u{2026}\u{201d} to point at the \
             folder you cloned.",
            home()
        )
    })?;
    let script = format!("{}/{}", dir, name);
    if !Path::new(&script).exists() {
        return Err(format!("{} not found in {}", name, dir));
    }


    let cmd = script_command(cfg, &dir, name);
    let script_line = applescript_escape(&cmd);
    let osa = format!(
        "tell application \"Terminal\"\n  activate\n  do script \"{}\"\nend tell",
        script_line
    );

    let out = Command::new("osascript")
        .arg("-e")
        .arg(&osa)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "Couldn't drive Terminal: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(format!(
        "Running {} in Terminal against {} — watch that window.",
        name,
        cfg.root()
    ))
}

// AppleScript string literals only need backslash and double-quote escaped.
fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

// Let the user point at the folder themselves when the search misses. osascript
// avoids pulling in the dialog plugin for one picker.
fn pick_repo(core: &Core) -> Result<String, String> {
    let out = sh(
        "osascript -e 'POSIX path of (choose folder with prompt \"Select the LTX Mac Farm scripts folder (the one containing farm_worker.sh)\")' 2>/dev/null",
    );
    let dir = out.trim().trim_end_matches('/').to_string();
    if dir.is_empty() {
        return Err("Cancelled.".into());
    }
    if !Path::new(&dir).join("farm_worker.sh").exists() {
        return Err(format!("{} doesn't contain farm_worker.sh.", dir));
    }
    let mut guard = core.cfg.lock().unwrap();
    guard.repo_dir = dir.clone();
    write_config(&guard)?;
    Ok(format!("Farm scripts: {}", dir))
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

fn discover_coordinators() -> Vec<String> {
    discover_smb_hosts()
}

#[derive(Serialize, Deserialize, Default)]
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
// Pure so --selftest can drive it for either role without Tauri state.
fn setup_steps_for(cfg: &Config) -> serde_json::Value {
    let cfg = cfg.clone();
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

    // Models move in OPPOSITE directions depending on the role, so this is two
    // different steps wearing one name. The coordinator already has the models
    // (it's the Mac that downloaded them) and PUSHES them to the share; every
    // worker PULLS them off the share. Showing a worker's "provision" button on
    // the coordinator would have it try to copy the models onto itself.
    let has_local_models = !list_dir_all(Path::new(&format!("{}/.cache/huggingface/hub", home())))
        .iter()
        .filter(|n| n.starts_with("models--"))
        .collect::<Vec<_>>()
        .is_empty();

    if coord {
        let staged = !list_dir_all(&Path::new(&root).join("models"))
            .iter()
            .filter(|n| n.starts_with("models--"))
            .collect::<Vec<_>>()
            .is_empty();
        push(&mut steps, "stage", "Put the models on the share",
            "Publishes this Mac's models + LoRAs to the shared folder so the other Macs \
             pull them over the switch instead of re-downloading ~87GB from HuggingFace. \
             The share is on the same disk, so this HARDLINKS rather than copying — \
             seconds, and no second copy of the models. Follows MANIFEST.txt.",
            staged,
            if staged { format!("Models staged in {}/models", root) }
            else if has_local_models { "Ready to stage — models are in this Mac's cache".into() }
            else { "No models in this Mac's cache yet. Run a render once, or copy them here first.".into() },
            "seed_assets", "Stage models on the share", false);
    } else {
        push(&mut steps, "models", "Copy the models to this Mac",
            "~60GB pulled off the share over the switch — far faster than HuggingFace.",
            has_local_models,
            if has_local_models { "Models present in the local HuggingFace cache".into() }
                 else { "No models cached locally yet".into() },
            "run_provision", "Provision", false);
    }

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
fn set_role(core: &Core, role: &str) -> Result<(), String> {
    core.bump();
    let mut guard = core.cfg.lock().unwrap();
    guard.role = role.to_string();
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

fn set_coordinator(core: &Core, name: &str) -> Result<(), String> {
    core.bump();
    let mut guard = core.cfg.lock().unwrap();
    guard.coordinator = name.to_string();
    if guard.share_path.trim().is_empty() {
        guard.share_path = format!("/Volumes/{}", guard.share_name.trim());
    }
    write_config(&guard)
}

fn finish_wizard(core: &Core) -> Result<(), String> {
    core.bump();
    let mut guard = core.cfg.lock().unwrap();
    guard.wizard_done = true;
    write_config(&guard)
}

// ---------------------------------------------------------------------------
// Watcher
// ---------------------------------------------------------------------------

fn state_json(core: &Core) -> serde_json::Value {
    let f = core.farm.lock().unwrap();
    // Workers ride along with the 2s poll rather than waiting for a verify
    // pass: which Macs are up and what they're rendering is the single most
    // useful thing on the Farm view, and read_workers is only a dir listing —
    // no shelling out, so it's cheap enough to run every tick.
    let workers = read_workers(&f.root);
    serde_json::json!({
        "root": f.root,
        "counts": f.counts,
        "workers": workers,
        "events": f.events.iter().rev().take(60).cloned().collect::<Vec<_>>(),
        // the UI reloads its config + views when this changes
        "rev": *core.rev.lock().unwrap(),
        "surface_host": this_host(),
    })
}

#[tauri::command]
fn show_dashboard(app: AppHandle) {
    if let Some(w) = app.get_webview_window("dash") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

// The poll loop, shared by the menubar app and `--serve`. `app` is None when
// there's no GUI (a headless render Mac running the gateway only), which turns
// off notifications, sounds and the tray tooltip but keeps counts, events and
// presence — everything the board and the Team view read.
fn watch_loop(core: Arc<Core>, app: Option<AppHandle>) {
    let dirs = ["queue", "running", "done", "failed"];
    let mut seen: HashMap<&str, HashSet<String>> = HashMap::new();
    let mut first = true;
    let mut cur_root = String::new();
    let mut last_presence = 0u64;
    let mut last_super = 0u64;
    let mut finished_runs: HashSet<String> = HashSet::new();
    let mut first_runs_pass = true;

    loop {
        // re-read the configured root every tick so Settings hot-reloads
        let root = core.cfg.lock().unwrap().root();
        if root != cur_root {
            cur_root = root.clone();
            seen.clear();
            first = true; // don't spam notifications for a folder we just switched to
            core.farm.lock().unwrap().root = root.clone();
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
                        "queue" => ("sent", "📤 Ping sent".to_string(), format!("Job “{}” queued", id), "Tink"),
                        "running" => ("received", "📥 Ping received".to_string(), format!("{} picked up “{}”", host, id), "Ping"),
                        "done" => ("done", "✅ Render done".to_string(), format!("{} finished “{}”", host, id), "Glass"),
                        "failed" => ("failed", "❌ Render failed".to_string(), format!("“{}” failed on {}", id, host), "Basso"),
                        _ => continue,
                    };
                    if let Some(a) = &app {
                        notify(a, &title, &body);
                        play(sound);
                    }
                    fresh.push(Event { kind: kind.to_string(), id, host, ts: now_ts() });
                }
            }
            // let requeued/re-appearing names fire again next time
            set.retain(|n| names.contains(n));
        }

        {
            let mut f = core.farm.lock().unwrap();
            f.counts = counts.clone();
            f.events.append(&mut fresh);
            let overflow = f.events.len().saturating_sub(200);
            if overflow > 0 {
                f.events.drain(0..overflow);
            }
        }
        if let Some(a) = &app {
            update_tray(a, &counts);
        }

        // Presence: tell the rest of the farm this Mac is here. Every ~10s, not
        // every 2s — it's an SMB write, and five Macs hammering the share with
        // heartbeats is exactly the kind of chatter that makes a queue folder
        // feel slow.
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        if now.saturating_sub(last_presence) >= 10 {
            last_presence = now;
            publish_presence(&core, now);
        }

        // --- the overnight shift ------------------------------------------
        // Once a minute, not every tick: these read the whole share, and nothing
        // they fix happens on a two-second timescale.
        if now.saturating_sub(last_super) >= 60 && Path::new(&root).is_dir() {
            last_super = now;
            supervise(&core, &app, &root, &mut finished_runs, &mut first_runs_pass);
        }

        first = false;
        std::thread::sleep(Duration::from_secs(2));
    }
}

// Autopilot + run completion, once a minute.
//
// SAFETY OF THE UNATTENDED PATH. Autopilot only ever requeues work — it never
// deletes a job, never touches a file a worker holds, and it stops the farm
// rather than looping on a fault. It also runs on ONE Mac: whoever holds the
// lock file on the share, refreshed every minute. Two babysitters would requeue
// the same failure twice and double the night's work.
fn supervise(
    core: &Arc<Core>,
    app: &Option<AppHandle>,
    root: &str,
    finished: &mut HashSet<String>,
    first_pass: &mut bool,
) {
    let cfg = core.cfg.lock().unwrap().clone();
    let host = this_host();

    if cfg.autopilot && jobs::claim_supervisor(root, &host, now_secs()) {
        let pol = jobs::AutoPolicy {
            stale_min: cfg.stale_min,
            max_retry: cfg.autopilot_retry,
            fail_streak: cfg.fail_streak,
            member: cfg.member.clone(),
        };
        let out = jobs::autopilot_tick(root, &pol, &job_stamp());
        if out.did_something() {
            let line = out.summary();
            jobs::log_autopilot(root, &host, &line);
            core.bump();
            if let Some(a) = app {
                // A pause is the one thing somebody has to know about — the farm
                // has stopped taking work and won't start again on its own.
                if out.paused {
                    notify(a, "⏸ Farm paused by autopilot", &format!("{}. Nothing new will start until you resume it.", out.reason));
                    play("Basso");
                } else {
                    notify(a, "🤖 Autopilot", &line);
                }
            }
        }
    }

    // Run completion. The first pass only records what's already finished, so
    // launching the app in the morning doesn't announce last night twice.
    let runs = jobs::runs(root);
    for r in &runs {
        let name = r["run"].as_str().unwrap_or("").to_string();
        if name.is_empty() || !r["finished"].as_bool().unwrap_or(false) {
            continue;
        }
        if finished.insert(name.clone()) && !*first_pass {
            let done = r["done"].as_u64().unwrap_or(0);
            let failed = r["failed"].as_u64().unwrap_or(0);
            let secs = r["render_secs"].as_u64().unwrap_or(0);
            let line = format!(
                "{} done, {} failed · {} of render time",
                done,
                failed,
                human_secs(secs)
            );
            jobs::log_autopilot(root, &host, &format!("run “{}” finished: {}", name, line));
            core.bump();
            if let Some(a) = app {
                notify(a, &format!("🌅 Run “{}” finished", name), &line);
                play("Glass");
            }
        }
    }
    *first_pass = false;
}

fn human_secs(n: u64) -> String {
    if n < 60 {
        return format!("{}s", n);
    }
    if n < 3600 {
        return format!("{}m", n / 60);
    }
    format!("{}h {}m", n / 3600, (n % 3600) / 60)
}

fn spawn_watcher(core: Arc<Core>, app: Option<AppHandle>) {
    std::thread::spawn(move || watch_loop(core, app));
}

// One file per Mac in <share>/presence/. Written by whoever is running the app;
// read by everyone's Team view. Deliberately last-write-wins with no locking:
// the only writer of <host>.json is that host.
fn publish_presence(core: &Core, now: u64) {
    let cfg = core.cfg.lock().unwrap().clone();
    let root = cfg.root();
    if !Path::new(&root).is_dir() {
        return; // share isn't mounted — nothing to publish to
    }
    let gateway = core
        .gateway
        .lock()
        .unwrap()
        .as_ref()
        .filter(|g| g.lan) // a 127.0.0.1 link is useless to anyone else
        .map(|g| g.lan_url(&this_host()))
        .unwrap_or_default();
    let p = jobs::Presence {
        host: this_host(),
        member: cfg.member.clone(),
        model: jobs::mac_model(),
        ram_gb: jobs::ram_gb(),
        role: if cfg.role.is_empty() { "worker".into() } else { cfg.role.clone() },
        perf: cfg.perf.clone(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        gateway,
        ts: now,
    };
    let _ = jobs::write_presence(&root, &p);
}

// ---------------------------------------------------------------------------
// The web gateway, from the app's side
// ---------------------------------------------------------------------------

fn gateway_json(core: &Core) -> serde_json::Value {
    let cfg = core.cfg.lock().unwrap().clone();
    let g = core.gateway.lock().unwrap();
    match g.as_ref() {
        Some(g) => serde_json::json!({
            "running": true,
            "enabled": cfg.web_enabled,
            "port": g.port,
            "lan": g.lan,
            "local_url": g.local_url(),
            "lan_url": g.lan_url(&this_host()),
            "token": g.token,
        }),
        None => serde_json::json!({
            "running": false,
            "enabled": cfg.web_enabled,
            "port": cfg.web_port,
            "lan": cfg.web_lan,
            "local_url": "",
            "lan_url": "",
            "token": "",
        }),
    }
}

// Bind (or re-bind) the gateway to whatever the config now says.
fn restart_gateway(core: &Core) {
    if let Some(old) = core.gateway.lock().unwrap().take() {
        old.stop();
    }
    let cfg = core.cfg.lock().unwrap().clone();
    if !cfg.web_enabled {
        return;
    }
    // start() hands this Arc to its serving threads, so they keep the Core alive
    // for as long as the gateway is up.
    let Some(arc) = core.arc() else { return };
    match web::start(arc, cfg.web_port, cfg.web_lan, cfg.web_token.clone()) {
        Ok(g) => {
            *core.gateway.lock().unwrap() = Some(g);
        }
        Err(e) => eprintln!("web gateway: {}", e),
    }
}

// ---------------------------------------------------------------------------
// Board, variants and team — the dispatch-facing wrappers
// ---------------------------------------------------------------------------

fn arg_str(args: &serde_json::Value, key: &str) -> String {
    args.get(key).and_then(|v| v.as_str()).unwrap_or("").trim().to_string()
}

// A stamp in enqueue.sh's shape, so a job created here sorts among the ones
// created in Terminal. Seconds resolution matches the shell's `date +%Y…%S`.
fn job_stamp() -> String {
    // No chrono in the tree, and pulling one in for one line isn't worth it.
    let out = sh("date +%Y%m%d_%H%M%S");
    let t = out.trim();
    if t.len() == 15 {
        t.to_string()
    } else {
        format!("00000001_{:06}", SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() % 1_000_000).unwrap_or(0))
    }
}

const STATS_TTL: u64 = 30;

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn stats_cached(core: &Core, root: &str) -> jobs::Stats {
    let now = now_secs();
    if let Some((ts, st)) = core.stats.lock().unwrap().as_ref() {
        if now.saturating_sub(*ts) < STATS_TTL {
            return st.clone();
        }
    }
    let st = jobs::stats(root);
    *core.stats.lock().unwrap() = Some((now, st.clone()));
    st
}

// Fill in "how long will this take" and "when does it start".
//
// The simulation is the honest part: each free Mac is a slot, a running job's
// slot frees when its estimate runs out, and queued jobs take the next slot in
// claim order. One Mac and four jobs means the fourth is four renders away, and
// the board should say so rather than showing the same ETA on all of them.
fn fill_estimates(b: &mut jobs::Board, st: &jobs::Stats, members: &[jobs::Member]) {
    let mut slots: Vec<u64> = Vec::new();
    for c in b.running.iter_mut() {
        c.est_secs = jobs::estimate_secs(st, c.width, c.height, c.frames, &c.mode);
        c.eta_secs = c.est_secs.saturating_sub(c.age_secs);
        slots.push(c.eta_secs);
    }
    // Macs that are up and idle can take work immediately.
    let idle = members
        .iter()
        .filter(|m| m.worker && m.state != "offline" && m.state != "paused" && m.state != "rendering")
        .count();
    for _ in 0..idle {
        slots.push(0);
    }
    if slots.is_empty() {
        slots.push(0); // nobody is up: show the queue as if one Mac will start
    }
    for c in b.queued.iter_mut() {
        c.est_secs = jobs::estimate_secs(st, c.width, c.height, c.frames, &c.mode);
        let (i, start) = slots
            .iter()
            .enumerate()
            .min_by_key(|(_, t)| **t)
            .map(|(i, t)| (i, *t))
            .unwrap_or((0, 0));
        c.eta_secs = start;
        slots[i] = start + c.est_secs;
    }
    for c in b.done.iter_mut() {
        c.est_secs = jobs::estimate_secs(st, c.width, c.height, c.frames, &c.mode);
    }
}

// Everything the Board view needs in one round trip: the lanes, plus how the
// farm is doing, so the browser polls once rather than three times.
fn board_json(core: &Core, cfg: &Config) -> serde_json::Value {
    let root = cfg.root();
    let mut b = jobs::board(&root, 60);
    let st = stats_cached(core, &root);
    let members = jobs::members(&root);
    fill_estimates(&mut b, &st, &members);
    serde_json::json!({
        "board": b,
        "share_url": cfg.share_url(),
        "held": jobs::held_count(&root),
        "member": cfg.member,
        "runs": jobs::runs(&root),
        "is_coordinator": !cfg.coordinator.trim().is_empty()
            && safe_host(&cfg.coordinator).eq_ignore_ascii_case(&this_host()),
    })
}

// One or many. The board's multi-select posts `files: [...]`, a single card
// posts `file: "..."`; everything else is identical, so they share one path.
fn job_action(core: &Core, args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let list: Vec<String> = args
        .get("files")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    if list.len() > 1 {
        let mut done = 0usize;
        let mut errs: Vec<String> = Vec::new();
        for f in &list {
            let mut one = args.clone();
            if let Some(o) = one.as_object_mut() {
                o.remove("files");
                o.insert("file".into(), serde_json::json!(f));
            }
            match job_action(core, &one) {
                Ok(_) => done += 1,
                Err(e) => errs.push(e),
            }
        }
        if done == 0 {
            return Err(errs.first().cloned().unwrap_or_else(|| "nothing changed".into()));
        }
        let mut msg = format!("{} job(s) updated", done);
        if !errs.is_empty() {
            msg = format!("{} · {} skipped ({})", msg, errs.len(), errs[0]);
        }
        return Ok(serde_json::json!({ "message": msg }));
    }

    let cfg = core.cfg.lock().unwrap().clone();
    let root = cfg.root();
    let action = arg_str(args, "action");
    let file = if list.len() == 1 { list[0].clone() } else { arg_str(args, "file") };
    let lane = arg_str(args, "lane");
    let msg = match action.as_str() {
        "promote" => jobs::set_priority(&root, &file, true)?,
        "demote" => jobs::set_priority(&root, &file, false)?,
        "cancel" => jobs::cancel_job(&root, &file)?,
        "requeue" => jobs::requeue_job(&root, &lane, &file, &job_stamp())?,
        "reorder" => {
            let order: Vec<String> = args
                .get("order")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default();
            if order.is_empty() {
                return Err("no order given".into());
            }
            jobs::reorder_queue(&root, &order)?
        }
        // Reveal on THIS Mac. Over the gateway that means the Mac hosting the
        // app, which is the right behaviour for the coordinator and honest
        // everywhere else — the browser also gets a direct /file link.
        "reveal" => {
            let path = web::safe_media_path(&root, &arg_str(args, "path"))
                .or_else(|_| reveal_job_path(&root, &lane, &file))?;
            let _ = Command::new("open").arg("-R").arg(&path).spawn();
            format!("Revealed {} in Finder on {}", path.display(), this_host())
        }
        // The two useful answers to a memory kill, straight off the failed card:
        // find a Mac that can afford it, or make the job smaller.
        "bigger_mac" | "smaller" => {
            let card = jobs::find_card(&root, if lane.is_empty() { "failed" } else { &lane }, &file)
                .ok_or_else(|| format!("{} isn't on the board any more.", file))?;
            let mut job: jobs::NewJob = serde_json::from_value(
                serde_json::to_value(&card).map_err(|e| e.to_string())?,
            )
            .unwrap_or_default();
            job.id = card.id.clone();
            job.prompt = card.prompt.clone();
            job.run = card.run.clone();
            job.member = cfg.member.clone();
            job.sweep = 0;
            job.priority = "normal".into();
            let note = if action == "bigger_mac" {
                let big = jobs::members(&root).iter().map(|m| m.ram_gb).max().unwrap_or(0);
                if big == 0 {
                    return Err("No Mac has reported its RAM yet — open the app on the render Macs first.".into());
                }
                job.min_ram_gb = big;
                format!("only Macs with {}GB+ may claim it", big)
            } else {
                job.width = (card.width * 2 / 3).max(544) / 8 * 8;
                job.height = (card.height * 2 / 3).max(544) / 8 * 8;
                job.perf = "light".into();
                format!("re-queued at {}×{} on the light profile", job.width, job.height)
            };
            jobs::enqueue(&root, &job, &job_stamp())?;
            let from = Path::new(&root).join("failed").join(&card.file);
            let to = Path::new(&root).join("failed").join(format!("retried_{}", card.file));
            let _ = std::fs::rename(from, to);
            format!("{} is back in the queue — {}.", card.id, note)
        }
        // A finished proof still, promoted to the real thing.
        "render_hero" => {
            let card = jobs::find_card(&root, "done", &file)
                .ok_or_else(|| format!("{} isn't in the done lane any more.", file))?;
            let mut job: jobs::NewJob = serde_json::from_value(
                serde_json::to_value(&card).map_err(|e| e.to_string())?,
            )
            .unwrap_or_default();
            job.id = card.id.trim_end_matches("_proof").to_string();
            job.prompt = card.prompt.clone();
            job.mode = "hero".into();
            job.member = cfg.member.clone();
            job.run = card.run.clone();
            job.sweep = 0;
            if job.prompt.trim().is_empty() {
                return Err("That proof has no prompt recorded, so it can't be re-rendered.".into());
            }
            jobs::enqueue(&root, &job, &job_stamp())?;
            format!("Queued the full render of {}.", job.id)
        }
        other => return Err(format!("unknown job action: {}", other)),
    };
    Ok(serde_json::json!({ "message": msg }))
}

// `reveal` on a job file rather than a render: still confined to the share.
fn reveal_job_path(root: &str, lane: &str, file: &str) -> Result<PathBuf, String> {
    let sub = match lane {
        "queued" => "queue",
        "running" => "running",
        "done" => "done",
        "failed" => "failed",
        _ => return Err("nothing to reveal".into()),
    };
    if file.is_empty() || file.contains('/') || file.contains("..") {
        return Err("not a job file name".into());
    }
    let p = Path::new(root).join(sub).join(file);
    let p = if p.is_file() { p } else { Path::new(root).join("queue/hi").join(file) };
    if p.is_file() {
        Ok(p)
    } else {
        Err(format!("{} isn't there any more — the board has moved on.", file))
    }
}

fn enqueue_job(core: &Core, args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let cfg = core.cfg.lock().unwrap().clone();
    let root = cfg.root();
    if !Path::new(&root).join("queue").is_dir() && std::fs::create_dir_all(Path::new(&root).join("queue")).is_err() {
        return Err(format!(
            "Can't reach the queue at {} — mount the share first (Checks → Connect share).",
            root
        ));
    }
    // Accept either {job:{…}} or the fields at the top level, because the
    // variant list posts a whole job object and the form posts fields.
    let raw = args.get("job").cloned().unwrap_or_else(|| args.clone());
    let mut job: jobs::NewJob = serde_json::from_value(raw).map_err(|e| format!("bad job: {}", e))?;
    // Whoever queued it, on the record. The board shows it and the Team view
    // counts it — "who asked for this?" is otherwise unanswerable next morning.
    if job.member.trim().is_empty() {
        job.member = cfg.member.clone();
    }
    let files = jobs::enqueue(&root, &job, &job_stamp())?;
    Ok(serde_json::json!({
        "files": files,
        "message": if files.len() == 1 {
            format!("Queued {}.", jobs::safe_id(&job.id))
        } else {
            format!("Queued {} jobs — the farm splits them across the Macs.", files.len())
        },
    }))
}

fn variants_json(core: &Core, args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let cfg = core.cfg.lock().unwrap().clone();
    let root = cfg.root();
    let lane = arg_str(args, "lane");
    let file = arg_str(args, "file");
    let card = jobs::find_card(&root, &lane, &file)
        .ok_or_else(|| format!("{} isn't on the board any more.", file))?;
    Ok(serde_json::json!({ "job": card, "variants": jobs::variants_for(&card) }))
}

fn members_json(core: &Core) -> serde_json::Value {
    let cfg = core.cfg.lock().unwrap().clone();
    let root = cfg.root();
    serde_json::json!({
        "you": this_host(),
        "member": cfg.member,
        "reachable": Path::new(&root).is_dir(),
        "members": jobs::members(&root),
    })
}

// --- review + proofs ---------------------------------------------------

fn set_review(core: &Core, args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let cfg = core.cfg.lock().unwrap().clone();
    let r = jobs::Review {
        id: arg_str(args, "id"),
        state: arg_str(args, "state"),
        by: cfg.member.clone(),
        note: arg_str(args, "note").chars().take(400).collect(),
        ts: now_secs(),
    };
    let msg = jobs::write_review(&cfg.root(), &r)?;
    Ok(serde_json::json!({ "message": msg }))
}

fn proofs_json(core: &Core) -> serde_json::Value {
    let cfg = core.cfg.lock().unwrap().clone();
    let root = cfg.root();
    let mut b = jobs::board(&root, 60);
    let st = stats_cached(core, &root);
    let members = jobs::members(&root);
    fill_estimates(&mut b, &st, &members);
    serde_json::json!({
        "proofs": jobs::proofs(&root, 120),
        // the finished clips, for the review grid next to the stills
        "clips": b.done,
        "reachable": Path::new(&root).is_dir(),
    })
}

// --- assets, stats, farm.conf, ops ------------------------------------

fn farm_action(core: &Core, args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let cfg = core.cfg.lock().unwrap().clone();
    let root = cfg.root();
    let action = arg_str(args, "action");
    let msg = match action.as_str() {
        "reap" => {
            let reaped = jobs::reap(&root, cfg.stale_min)?;
            if reaped.is_empty() {
                "Nothing was stalled — every in-flight job has a live worker.".to_string()
            } else {
                jobs::log_autopilot(&root, &this_host(), &format!("manual reap: {}", reaped.join(", ")));
                format!("Requeued {} stalled job(s): {}", reaped.len(), reaped.join(", "))
            }
        }
        "pause" => {
            let n = jobs::pause_queue(&root)?;
            jobs::log_autopilot(&root, &this_host(), &format!("queue paused by hand ({} held)", n));
            format!("Paused — {} waiting job(s) held. Anything already rendering finishes.", n)
        }
        "resume" => {
            let n = jobs::resume_queue(&root)?;
            jobs::log_autopilot(&root, &this_host(), &format!("queue resumed by hand ({} released)", n));
            format!("Resumed — {} job(s) back in the queue.", n)
        }
        other => return Err(format!("unknown farm action: {}", other)),
    };
    core.bump();
    Ok(serde_json::json!({ "message": msg }))
}

// --- overnight runs ---------------------------------------------------

fn plan_run(core: &Core, args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let cfg = core.cfg.lock().unwrap().clone();
    let root = cfg.root();
    let raw = args.get("plan").cloned().unwrap_or_else(|| args.clone());
    let mut plan: jobs::RunPlan =
        serde_json::from_value(raw).map_err(|e| format!("bad plan: {}", e))?;
    if plan.member.trim().is_empty() {
        plan.member = cfg.member.clone();
    }
    let out = jobs::plan_run(&root, &plan, &job_stamp())?;
    // Stamp the manifest's clock here, where the clock lives.
    if let Some(run) = out["run"].as_str() {
        let p = jobs::runs_dir(&root).join(format!("{}.json", jobs::safe_id(run)));
        if let Ok(body) = std::fs::read_to_string(&p) {
            if let Ok(mut m) = serde_json::from_str::<jobs::RunManifest>(&body) {
                m.created_ts = now_secs();
                let _ = jobs::write_run(&root, &m);
            }
        }
        jobs::log_autopilot(
            &root,
            &this_host(),
            &format!("run “{}” planned: {} job(s) by {}", run, out["queued"], cfg.member),
        );
    }
    core.bump();
    Ok(out)
}

// --- autopilot --------------------------------------------------------

fn autopilot_json(core: &Core) -> serde_json::Value {
    let cfg = core.cfg.lock().unwrap().clone();
    let root = cfg.root();
    let lock = jobs::runs_dir(&root).join(".autopilot.lock");
    let holder = std::fs::read_to_string(&lock)
        .ok()
        .and_then(|b| serde_json::from_str::<serde_json::Value>(&b).ok())
        .and_then(|v| v["host"].as_str().map(|s| s.to_string()))
        .unwrap_or_default();
    let fresh = lock.is_file() && mtime_age(&lock) < 120;
    serde_json::json!({
        "on": cfg.autopilot,
        "you": this_host(),
        "supervisor": if fresh { holder } else { String::new() },
        "policy": {
            "retry": cfg.autopilot_retry,
            "stale_min": cfg.stale_min,
            "fail_streak": cfg.fail_streak,
        },
        "held": jobs::held_count(&root),
        "log": jobs::autopilot_log_tail(&root, 40),
    })
}

fn set_autopilot(core: &Core, args: &serde_json::Value) -> Result<serde_json::Value, String> {
    {
        let mut cfg = core.cfg.lock().unwrap();
        if let Some(on) = args.get("on").and_then(|v| v.as_bool()) {
            cfg.autopilot = on;
        }
        if let Some(n) = args.get("retry").and_then(|v| v.as_u64()) {
            cfg.autopilot_retry = (n as u32).min(5);
        }
        if let Some(n) = args.get("stale_min").and_then(|v| v.as_u64()) {
            cfg.stale_min = n.clamp(5, 240);
        }
        if let Some(n) = args.get("fail_streak").and_then(|v| v.as_u64()) {
            cfg.fail_streak = (n as u32).clamp(2, 50);
        }
        write_config(&cfg)?;
    }
    core.bump();
    let cfg = core.cfg.lock().unwrap().clone();
    let root = cfg.root();
    if Path::new(&root).is_dir() {
        jobs::log_autopilot(
            &root,
            &this_host(),
            if cfg.autopilot { "autopilot ON for this Mac" } else { "autopilot off for this Mac" },
        );
    }
    Ok(autopilot_json(core))
}

// --- presets ----------------------------------------------------------

fn save_preset(core: &Core, args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let name = arg_str(args, "name");
    if name.is_empty() {
        return Err("Give the preset a name.".into());
    }
    let job = args.get("job").cloned().ok_or("no job to save")?;
    {
        let mut cfg = core.cfg.lock().unwrap();
        cfg.presets.retain(|p| p["name"].as_str() != Some(name.as_str()));
        cfg.presets.push(serde_json::json!({ "name": name, "job": job }));
        if cfg.presets.len() > 40 {
            cfg.presets.remove(0);
        }
        write_config(&cfg)?;
    }
    core.bump();
    Ok(serde_json::json!({ "message": format!("Saved “{}”.", name),
        "presets": core.cfg.lock().unwrap().presets.clone() }))
}

fn delete_preset(core: &Core, args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let name = arg_str(args, "name");
    {
        let mut cfg = core.cfg.lock().unwrap();
        cfg.presets.retain(|p| p["name"].as_str() != Some(name.as_str()));
        write_config(&cfg)?;
    }
    core.bump();
    Ok(serde_json::json!({ "message": format!("Deleted “{}”.", name),
        "presets": core.cfg.lock().unwrap().presets.clone() }))
}

fn set_member(core: &Core, name: &str) -> Result<serde_json::Value, String> {
    core.bump();
    {
        let mut guard = core.cfg.lock().unwrap();
        guard.member = name.trim().chars().take(40).collect();
        write_config(&guard)?;
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    publish_presence(core, now);
    Ok(serde_json::json!({ "member": core.cfg.lock().unwrap().member }))
}

// ---------------------------------------------------------------------------
// Core::dispatch — the ONE command surface
// ---------------------------------------------------------------------------

// Every command name the UI may call, in one list so --selftest can prove the
// dispatch table and the UI agree. A name here that dispatch doesn't handle (or
// a call in the UI that isn't here) is a dead button, which is how this app has
// broken before.
pub(crate) const COMMANDS: [&str; 33] = [
    "get_state", "verify_link", "get_config", "save_config", "run_action",
    "setup_steps", "discover_coordinators", "set_role", "set_coordinator",
    "finish_wizard", "pick_repo", "mount_share",
    "get_board", "job_action", "enqueue_job", "job_variants", "get_members", "set_member",
    // review + proofs
    "get_proofs", "set_review",
    // scale, assets, numbers
    "list_assets", "get_stats", "get_job_log",
    // farm-wide operations
    "get_farm_conf", "save_farm_conf", "farm_action",
    // overnight runs + autopilot
    "plan_run", "get_runs", "get_run_report", "get_autopilot", "set_autopilot",
    "save_preset", "delete_preset",
];

impl Core {
    pub(crate) fn dispatch(&self, cmd: &str, args: &serde_json::Value) -> Result<serde_json::Value, String> {
        let cfg = || self.cfg.lock().unwrap().clone();
        match cmd {
            "get_state" => Ok(state_json(self)),
            "verify_link" => Ok(serde_json::to_value(verify_link(&cfg())).unwrap_or_default()),
            "get_config" => Ok(get_config_json(self)),
            "save_config" => {
                let raw = args.get("cfg").cloned().ok_or("no cfg given")?;
                let next = merge_config(&cfg(), &raw)?;
                save_config_core(self, next)
            }
            "run_action" => Ok(serde_json::json!(run_action(&arg_str(args, "action"), &cfg())?)),
            "mount_share" => Ok(serde_json::json!(do_mount(&cfg())?)),
            "setup_steps" => Ok(setup_steps_for(&cfg())),
            "discover_coordinators" => Ok(serde_json::json!(discover_coordinators())),
            "set_role" => {
                set_role(self, &arg_str(args, "role"))?;
                Ok(serde_json::Value::Null)
            }
            "set_coordinator" => {
                set_coordinator(self, &arg_str(args, "name"))?;
                Ok(serde_json::Value::Null)
            }
            "finish_wizard" => {
                finish_wizard(self)?;
                Ok(serde_json::Value::Null)
            }
            "pick_repo" => Ok(serde_json::json!(pick_repo(self)?)),
            "get_board" => Ok(board_json(self, &cfg())),
            "get_proofs" => Ok(proofs_json(self)),
            "set_review" => set_review(self, args),
            "list_assets" => Ok(jobs::list_assets(&cfg().root())),
            "get_stats" => {
                let c = cfg();
                let root = c.root();
                let st = stats_cached(self, &root);
                Ok(serde_json::json!({
                    "stats": st,
                    "members": jobs::members(&root),
                    "reachable": Path::new(&root).is_dir(),
                }))
            }
            "get_job_log" => jobs::log_tail(
                &cfg().root(),
                &arg_str(args, "id"),
                &arg_str(args, "host"),
                args.get("lines").and_then(|v| v.as_u64()).unwrap_or(200) as usize,
            ),
            "get_farm_conf" => Ok(jobs::read_farm_conf(&cfg().root())),
            "save_farm_conf" => {
                let patch = args.get("keys").cloned().unwrap_or_else(|| args.clone());
                let msg = jobs::save_farm_conf(&cfg().root(), &patch)?;
                self.bump();
                Ok(serde_json::json!({ "message": msg }))
            }
            "farm_action" => farm_action(self, args),
            "plan_run" => plan_run(self, args),
            "get_runs" => Ok(serde_json::json!({ "runs": jobs::runs(&cfg().root()) })),
            "get_run_report" => Ok(jobs::run_report(&cfg().root(), &arg_str(args, "run"))),
            "get_autopilot" => Ok(autopilot_json(self)),
            "set_autopilot" => set_autopilot(self, args),
            "save_preset" => save_preset(self, args),
            "delete_preset" => delete_preset(self, args),
            "job_action" => job_action(self, args),
            "enqueue_job" => enqueue_job(self, args),
            "job_variants" => variants_json(self, args),
            "get_members" => Ok(members_json(self)),
            "set_member" => set_member(self, &arg_str(args, "name")),
            other => Err(format!("unknown command: {}", other)),
        }
    }
}

// The only two Tauri commands left. `bridge` is the popover's door into
// dispatch; the gateway's POST /api/invoke is the other door to the same room.
#[tauri::command]
fn bridge(
    cmd: String,
    args: Option<serde_json::Value>,
    core: State<Arc<Core>>,
) -> Result<serde_json::Value, String> {
    core.dispatch(&cmd, &args.unwrap_or(serde_json::Value::Null))
}

// ---------------------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
// ---------------------------------------------------------------------------
// --selftest — exercise every wizard path headlessly.
//
// Written because four separate setup bugs (EACCES on /Volumes, the one-level
// repo scan, the clobbered FARM_ROOT, the Gatekeeper block) all reached the
// user first: each one only surfaces when a tray button is clicked, and the
// tray can't be driven from a terminal. This checks the same code the buttons
// run, for BOTH roles, without launching anything.
// ---------------------------------------------------------------------------

// The UI's own list of commands, parsed out of ui-react/src/commands.ts. Reading
// the source beats duplicating the list in Rust: there's exactly one place to
// edit when a command is added, and this notices if it wasn't edited.
fn ui_command_list() -> Option<Vec<String>> {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = here.join("../ui-react/src/commands.ts");
    let body = std::fs::read_to_string(path).ok()?;
    let start = body.find("COMMANDS = [")? + "COMMANDS = [".len();
    let end = body[start..].find(']')? + start;
    let names: Vec<String> = body[start..end]
        .split(',')
        .filter_map(|part| {
            let t = part.trim().trim_matches(|c| c == '\'' || c == '"' || c == '\n');
            (!t.is_empty() && t.chars().all(|c| c.is_ascii_lowercase() || c == '_')).then(|| t.to_string())
        })
        .collect();
    (!names.is_empty()).then_some(names)
}

// The built JS, for checking a stale bundle.
fn bundle_js() -> Option<String> {
    web::UI_FILES
        .iter()
        .find(|(r, _, _)| r.ends_with(".js"))
        .map(|(_, b, _)| String::from_utf8_lossy(b).to_string())
}

struct Report { pass: u32, fail: u32, warn: u32 }

impl Report {
    fn ok(&mut self, what: &str, detail: &str) {
        self.pass += 1;
        println!("  \x1b[32m✓\x1b[0m {:<44} {}", what, detail);
    }
    fn bad(&mut self, what: &str, detail: &str) {
        self.fail += 1;
        println!("  \x1b[31m✗\x1b[0m {:<44} {}", what, detail);
    }
    fn meh(&mut self, what: &str, detail: &str) {
        self.warn += 1;
        println!("  \x1b[33m!\x1b[0m {:<44} {}", what, detail);
    }
    fn check(&mut self, cond: bool, what: &str, detail: &str) {
        if cond { self.ok(what, detail) } else { self.bad(what, detail) }
    }
}

fn selftest_role(r: &mut Report, cfg: &Config, repo: Option<&String>) {
    let role = cfg.role.clone();
    println!("\n\x1b[1m── role: {} ──\x1b[0m", role);
    let root = cfg.root();

    // The EACCES bug: a coordinator must never resolve to the /Volumes mount point.
    if role == "coordinator" {
        r.check(!root.starts_with("/Volumes/"),
            "root is local, not a mount point", &root);
        // and it must actually be writable, which is the thing EACCES was about
        let probe = Path::new(&root).join(".ltx_selftest_write");
        match std::fs::create_dir_all(&root).and_then(|_| std::fs::write(&probe, b"x")) {
            Ok(_) => { let _ = std::fs::remove_file(&probe); r.ok("farm root is writable", &root); }
            Err(e) => r.bad("farm root is writable", &format!("{} — {}", root, e)),
        }
    } else {
        r.check(root.starts_with("/Volumes/"),
            "root is the mounted share", &root);
    }

    // Every step the wizard will render, and whether its button can work.
    let steps = selftest_steps(cfg);
    r.check(!steps.is_empty(), "wizard produced steps", &format!("{} steps", steps.len()));
    for st in &steps {
        if st.action.is_empty() { continue; }
        // A wizard action the backend doesn't handle = a dead button.
        r.check(KNOWN_ACTIONS.contains(&st.action.as_str()),
            &format!("action wired: {}", st.action),
            if KNOWN_ACTIONS.contains(&st.action.as_str()) { "handled" } else { "NOT HANDLED by run_action" });

        // For the three that shell out, prove the script exists and the command
        // we'd hand Terminal carries THIS role's config.
        let script = match st.action.as_str() {
            "run_setup" => Some("setup.command"),
            "run_provision" => Some("provision.command"),
            "seed_assets" => Some("seed_farm_assets.sh"),
            "start_worker" => Some("start_worker.command"),
            _ => None,
        };
        if let (Some(name), Some(dir)) = (script, repo) {
            let path = format!("{}/{}", dir, name);
            if !Path::new(&path).exists() {
                r.bad(&format!("script present: {}", name), &path);
                continue;
            }
            let cmd = script_command(cfg, dir, name);
            let carries_root = cmd.contains(&format!("FARM_ROOT={}", shell_quote(&root)));
            r.check(carries_root, &format!("{} gets this FARM_ROOT", name),
                if carries_root { &root } else { "MISSING — script would use its own default" });
            // and it must survive the trip into an AppleScript string literal
            let esc = applescript_escape(&cmd);
            r.check(!esc.contains('\u{0022}') || esc.contains("\\\""),
                &format!("{} escapes for Terminal", name), "no bare quotes");
            let syn = sh(&format!("bash -n {} 2>&1", shell_quote(&path)));
            r.check(syn.trim().is_empty(), &format!("{} parses", name),
                if syn.trim().is_empty() { "clean" } else { syn.trim() });
        }
    }
}

// setup_steps() minus the Tauri State wrapper, so the self-test can call it.
fn selftest_steps(cfg: &Config) -> Vec<SetupStep> {
    let v = setup_steps_for(cfg);
    serde_json::from_value(v["steps"].clone()).unwrap_or_default()
}

pub fn selftest() -> i32 {
    println!("\x1b[1mLTX Mac Farm — self test\x1b[0m");
    let mut r = Report { pass: 0, fail: 0, warn: 0 };
    let base = load_config();

    println!("\n\x1b[1m── environment ──\x1b[0m");
    println!("  host {}  ·  {}GB RAM", this_host(), sh("sysctl -n hw.memsize").trim().parse::<u64>().unwrap_or(0) / 1024/1024/1024);

    let repo = detect_repo(&base);
    match &repo {
        Some(d) => r.ok("farm scripts found", d),
        None => r.bad("farm scripts found", "detect_repo() returned nothing — every script button is dead"),
    }

    // Terminal must be drivable, or every shell-out button silently does nothing.
    let osa = sh("osascript -e 'tell application \"System Events\" to return name of application \"Terminal\"' 2>&1");
    r.check(!osa.to_lowercase().contains("not allowed") && !osa.to_lowercase().contains("error"),
        "can drive Terminal via osascript", osa.trim());

    let hosts = discover_smb_hosts();
    if hosts.is_empty() {
        r.meh("Bonjour finds another SMB host",
            "none besides this Mac — fine on the coordinator, but a worker would have nothing to pick");
    } else {
        r.ok("Bonjour finds an SMB host", &hosts.join(", "));
    }

    // Script-level checks. Both of these shipped as real bugs: the literal
    // "COORDINATOR.local" placeholder (each script had its own copy of the
    // mount logic, so fixing one missed the others), and --info=progress2,
    // which macOS's openrsync rejects outright rather than ignoring.
    if let Some(dir) = &repo {
        println!("\n\x1b[1m── scripts ──\x1b[0m");
        for f in ["start_worker.command", "setup.command", "provision.command", "seed_farm_assets.sh"] {
            let path = format!("{}/{}", dir, f);
            let body = std::fs::read_to_string(&path).unwrap_or_default();
            if body.is_empty() { r.bad(&format!("readable: {}", f), &path); continue; }
            r.check(!body.contains("COORDINATOR.local"),
                &format!("{}: no hardcoded coordinator", f),
                if body.contains("COORDINATOR.local") { "still has the smb://COORDINATOR.local placeholder" } else { "ok" });
            r.check(!body.contains("--info=progress2"),
                &format!("{}: rsync flag is portable", f),
                if body.contains("--info=progress2") { "openrsync on macOS rejects --info=progress2" } else { "ok" });
        }
        // and the shared helper must exist and actually be sourced
        let helper = format!("{}/farm_root.sh", dir);
        r.check(Path::new(&helper).exists(), "farm_root.sh present", &helper);
        let rp = sh("rsync --info=progress2 --version >/dev/null 2>&1 && echo gnu || echo openrsync");
        r.ok("rsync flavour detected", rp.trim());
    }

    // Both roles, regardless of how this Mac is currently configured.
    for role in ["coordinator", "worker"] {
        let mut c = base.clone();
        c.role = role.to_string();
        c.share_path = String::new();
        if role == "worker" && c.coordinator.trim().is_empty() {
            c.coordinator = "example-mac".into();
        }
        c.normalize();
        selftest_role(&mut r, &c, repo.as_ref());
    }

    // The healing path for installs broken by the original EACCES bug.
    println!("\n\x1b[1m── config repair ──\x1b[0m");
    let mut broken = base.clone();
    broken.role = "coordinator".into();
    broken.share_path = "/Volumes/RenderFarm".into();
    broken.normalize();
    r.check(!broken.share_path.starts_with("/Volumes/"),
        "heals a coordinator stuck on /Volumes", &broken.share_path);

    // --- the web gateway ---------------------------------------------------
    // A dead gateway is invisible from the tray (the menu item just does
    // nothing), so prove the port binds and the page it would serve is intact.
    println!("\n\x1b[1m── web gateway ──\x1b[0m");
    let gcfg = load_config();
    match std::net::TcpListener::bind(("127.0.0.1", gcfg.web_port)) {
        Ok(l) => {
            drop(l);
            r.ok("gateway port is free", &format!("127.0.0.1:{}", gcfg.web_port));
        }
        Err(_) => r.meh(
            "gateway port is free",
            &format!("{} is in use — the gateway will move up to the next free port", gcfg.web_port),
        ),
    }
    r.check(gcfg.web_token.len() == 32, "gateway key generated", &format!("{} chars", gcfg.web_token.len()));
    let built = web::page().is_some();
    r.check(built, "frontend is built into this binary",
        if built { "index.html embedded" } else { "run `npm run build` in desktop/ui-react" });
    if built {
        let html = web::page().map(|b| String::from_utf8_lossy(b).to_string()).unwrap_or_default();
        r.check(html.contains("LTX Mac Farm"), "served page is ours", "title present");
    }
    if gcfg.web_lan {
        r.meh("LAN sharing", "ON — anyone on this network with the key can drive this Mac");
    } else {
        r.ok("LAN sharing", "off — gateway is 127.0.0.1 only");
    }

    // --- command surface ----------------------------------------------------
    // The dead-button check, now across the language boundary: the UI declares
    // every command it may call in ui-react/src/commands.ts, `call()` only
    // accepts one of those (so a typo is a compile error), and this proves that
    // list and Core::dispatch's list are the same set. A name in one and not the
    // other is a button that spins forever — how this app broke four times.
    println!("\n\x1b[1m── command surface ──\x1b[0m");
    match ui_command_list() {
        Some(ui) => {
            r.ok("read the UI's command list", &format!("{} declared", ui.len()));
            for name in &ui {
                let known = COMMANDS.contains(&name.as_str());
                r.check(known, &format!("UI command exists: {}", name),
                    if known { "handled by Core::dispatch" } else { "NOT in Core::dispatch — dead button" });
            }
            for name in COMMANDS {
                if !ui.iter().any(|u| u == name) {
                    r.meh(&format!("unused command: {}", name), "dispatch handles it, the UI never calls it");
                }
            }
            // and the built bundle must actually contain them, or the build is stale
            if let Some(bundle) = bundle_js() {
                let missing: Vec<&String> = ui.iter().filter(|n| !bundle.contains(n.as_str())).collect();
                r.check(missing.is_empty(), "built bundle matches the command list",
                    &if missing.is_empty() { "every command appears in the bundle".to_string() }
                     else { format!("stale build — missing {:?}", missing) });
            }
        }
        None => r.bad("read the UI's command list",
            "couldn't parse ui-react/src/commands.ts — the dead-button check can't run"),
    }

    println!("\n\x1b[1m{} passed · {} failed · {} warnings\x1b[0m", r.pass, r.fail, r.warn);
    if r.fail == 0 { println!("\x1b[32mAll wizard paths are wired.\x1b[0m"); 0 } else { println!("\x1b[31mFix the ✗ rows above.\x1b[0m"); 1 }
}

pub fn run() {
    let cfg = load_config();
    let root = cfg.root();
    let core = Core::new_arc(cfg, Farm { root, ..Default::default() });

    // Up before the window exists: a Mac left on the login screen with the app
    // running should still be reachable at its gateway.
    restart_gateway(&core);
    // And, if asked, open it. The browser view is the one people actually work
    // in — the popover is the glance — so launching straight into it is the
    // default. Off with one checkbox in Settings → Web gateway.
    {
        let cfg = core.cfg.lock().unwrap().clone();
        if cfg.web_enabled && cfg.web_open_on_launch {
            if let Some(g) = core.gateway.lock().unwrap().as_ref() {
                let _ = Command::new("open").arg(g.local_url()).spawn();
            }
        }
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .manage(core.clone())
        .invoke_handler(tauri::generate_handler![bridge, show_dashboard])
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let show = MenuItem::with_id(app, "show", "Open dashboard", true, None::<&str>)?;
            let board = MenuItem::with_id(app, "board", "Job board…", true, None::<&str>)?;
            let setup = MenuItem::with_id(app, "setup", "Setup & Verify…", true, None::<&str>)?;
            let sep0 = PredefinedMenuItem::separator(app)?;
            // The two gateway items are the whole point of the web surface: one
            // opens it here, one hands the link to somebody else.
            let webui = MenuItem::with_id(app, "web_open", "Open in browser", true, None::<&str>)?;
            let weblink = MenuItem::with_id(app, "web_copy", "Copy team link", true, None::<&str>)?;
            let sep1 = PredefinedMenuItem::separator(app)?;
            let openf = MenuItem::with_id(app, "open_folder", "Reveal farm folder", true, None::<&str>)?;
            let sep2 = PredefinedMenuItem::separator(app)?;
            let quit = PredefinedMenuItem::quit(app, Some("Quit LTX Mac Farm"))?;
            let menu = Menu::with_items(
                app,
                &[&show, &board, &setup, &sep0, &webui, &weblink, &sep1, &openf, &sep2, &quit],
            )?;

            let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;
            let _tray = TrayIconBuilder::with_id("main")
                .icon(icon)
                .icon_as_template(true)
                .tooltip("LTX Mac Farm")
                .menu(&menu)
                .on_menu_event(|app, event| {
                    let core: State<Arc<Core>> = app.state();
                    match event.id().as_ref() {
                        "show" | "setup" | "board" => {
                            if let Some(w) = app.get_webview_window("dash") {
                                // ask the UI which tab to land on
                                let tab = match event.id().as_ref() {
                                    "setup" => "wiz",
                                    "board" => "board",
                                    _ => "dash",
                                };
                                let _ = w.eval(&format!("window.__openTab && window.__openTab('{}')", tab));
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                        }
                        "web_open" => match core.gateway.lock().unwrap().as_ref() {
                            Some(g) => {
                                let _ = Command::new("open").arg(g.local_url()).spawn();
                            }
                            None => notify(
                                app,
                                "Web gateway is off",
                                "Turn it on in Checks → Settings → Web gateway.",
                            ),
                        },
                        "web_copy" => {
                            let (msg, body) = match core.gateway.lock().unwrap().as_ref() {
                                Some(g) if g.lan => (
                                    "Team link copied".to_string(),
                                    g.lan_url(&this_host()),
                                ),
                                Some(g) => (
                                    "Copied — this Mac only".to_string(),
                                    g.local_url(),
                                ),
                                None => ("Web gateway is off".to_string(), String::new()),
                            };
                            if body.is_empty() {
                                notify(app, &msg, "Turn it on in Checks → Settings → Web gateway.");
                            } else {
                                copy_to_clipboard(&body);
                                notify(app, &msg, &body);
                            }
                        }
                        "open_folder" => {
                            let root = core.cfg.lock().unwrap().root();
                            let _ = Command::new("open").arg(root).spawn();
                        }
                        _ => {}
                    }
                })
                .build(app)?;

            let core: State<Arc<Core>> = app.state();
            spawn_watcher(core.inner().clone(), Some(app.handle().clone()));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running LTX Mac Farm");
}

fn copy_to_clipboard(text: &str) {
    use std::io::Write;
    use std::process::Stdio;
    // pbcopy rather than a clipboard crate: one dependency-free line, and it's
    // the same thing every other script on this Mac uses.
    if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
        if let Some(mut si) = child.stdin.take() {
            let _ = si.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}

/// `ltx-mac-farm --serve` — the gateway with no menubar and no window.
///
/// For the Macs nobody sits at: a render node in a cupboard still shows up in
/// everyone's Team view, still publishes its presence, and can still be set up
/// and driven from a browser. Runs in the foreground so launchd/Terminal owns it.
pub fn serve() -> i32 {
    let cfg = load_config();
    let root = cfg.root();
    let port = cfg.web_port;
    let lan = cfg.web_lan;
    let core = Core::new_arc(cfg, Farm { root: root.clone(), ..Default::default() });

    let cfg_now = core.cfg.lock().unwrap().clone();
    match web::start(core.clone(), port, lan, cfg_now.web_token.clone()) {
        Ok(g) => {
            println!("LTX Mac Farm gateway on {}", g.local_url());
            if g.lan {
                println!("team link            {}", g.lan_url(&this_host()));
            } else {
                println!("(this Mac only — tick “share on the LAN” in Settings to let the team in)");
            }
            println!("farm folder          {}", root);
            println!("Ctrl-C to stop.");
            *core.gateway.lock().unwrap() = Some(g);
        }
        Err(e) => {
            eprintln!("!! {}", e);
            return 1;
        }
    }
    // The poll loop is what keeps counts, events and presence live; without a
    // GUI it never returns, which is exactly the behaviour a service wants.
    watch_loop(core, None);
    0
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

    // The exact layout that produced "Farm folder not found": the repo sits two
    // levels under ~/Desktop, and the old scan only looked one level down.
    #[test]
    fn finds_a_repo_nested_two_levels_deep() {
        let base = std::env::temp_dir().join("ltxtest_nested");
        let deep = base.join("00 - Aidxn").join("LTX Mac Farm (VideoGen)");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("farm_worker.sh"), "#!/bin/bash\n").unwrap();
        let found = find_repo_under(&base, 3).expect("should find it two levels down");
        assert_eq!(found, deep.to_string_lossy());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn repo_search_prunes_heavy_dirs_and_respects_depth() {
        let base = std::env::temp_dir().join("ltxtest_prune");
        let _ = std::fs::remove_dir_all(&base);
        // hidden inside node_modules -> must NOT be found
        let nm = base.join("node_modules").join("pkg");
        std::fs::create_dir_all(&nm).unwrap();
        std::fs::write(nm.join("farm_worker.sh"), "x").unwrap();
        // deeper than the limit -> must NOT be found
        let too_deep = base.join("a").join("b").join("c").join("d");
        std::fs::create_dir_all(&too_deep).unwrap();
        std::fs::write(too_deep.join("farm_worker.sh"), "x").unwrap();
        assert_eq!(find_repo_under(&base, 3), None);
        let _ = std::fs::remove_dir_all(&base);
    }

    // Paths with spaces AND quotes must survive the shell->AppleScript trip.
    #[test]
    fn applescript_escaping_is_safe() {
        assert_eq!(applescript_escape(r#"a"b"#), r#"a\"b"#);
        assert_eq!(applescript_escape(r"a\b"), r"a\\b");
        // the real-world case: the repo path has spaces, parens and a dash
        let q = shell_quote("/Users/a/Desktop/00 - Aidxn/LTX Mac Farm (VideoGen)");
        let esc = applescript_escape(&format!("cd {} && ./x.command", q));
        assert!(!esc.contains('\u{0022}'), "no bare double quotes may reach AppleScript: {}", esc);
        assert!(esc.contains("00 - Aidxn"));
    }

    // A test Core pointed at a throwaway share, with HOME redirected so config
    // and presence writes can't touch the real install.
    fn test_core(name: &str) -> (Arc<Core>, PathBuf) {
        let tmp = std::env::temp_dir().join(format!("ltxcore_{}", name));
        let _ = std::fs::remove_dir_all(&tmp);
        let share = tmp.join("RenderFarm");
        for d in ["queue", "queue/hi", "running", "done", "failed", "presence"] {
            std::fs::create_dir_all(share.join(d)).unwrap();
        }
        // One shared config dir for every test in this file: never the real
        // install, and the same value in every thread, so a parallel test can't
        // see it change mid-assertion.
        let cfgdir = std::env::temp_dir().join("ltxtest_config");
        std::fs::create_dir_all(&cfgdir).unwrap();
        std::env::set_var("FARM_CONFIG_DIR", &cfgdir);
        let cfg = Config {
            role: "coordinator".into(),
            coordinator: this_host(),
            share_path: share.to_string_lossy().to_string(),
            web_token: "0".repeat(32),
            member: "Test Person".into(),
            ..Default::default()
        };
        let root = cfg.root();
        (Core::new_arc(cfg, Farm { root, ..Default::default() }), share)
    }

    // The UI declares its commands in TypeScript; dispatch declares them in Rust.
    // Neither list is generated from the other, so this is the seam that would
    // rot silently — a name added to one and not the other is a dead button.
    #[test]
    fn the_ui_and_dispatch_agree_on_the_command_list() {
        let ui = ui_command_list().expect("ui-react/src/commands.ts must be readable");
        for name in &ui {
            assert!(
                COMMANDS.contains(&name.as_str()),
                "{} is declared by the UI but not handled by Core::dispatch",
                name
            );
        }
        for name in COMMANDS {
            assert!(
                ui.iter().any(|u| u == name),
                "{} is handled by dispatch but the UI never declares it — remove it or call it",
                name
            );
        }
        assert_eq!(ui.len(), COMMANDS.len());
    }

    // Saving one form must not reset the fields another form owns. Before this
    // merged, posting a port number wiped role + wizard_done and the app
    // reopened the setup wizard as if the Mac had never been configured.
    #[test]
    fn saving_settings_keeps_the_fields_the_form_didnt_send() {
        let current = Config {
            role: "coordinator".into(),
            wizard_done: true,
            member: "Aiden".into(),
            web_token: "a".repeat(32),
            web_port: 8787,
            coordinator: "mac-studio".into(),
            ..Default::default()
        };
        // what the gateway form posts: its own three fields, nothing else
        let patch = serde_json::json!({ "web_port": 9100, "web_lan": true, "web_enabled": true });
        let merged = merge_config(&current, &patch).unwrap();
        assert_eq!(merged.web_port, 9100);
        assert!(merged.web_lan);
        assert_eq!(merged.role, "coordinator", "role must survive");
        assert!(merged.wizard_done, "wizard_done must survive");
        assert_eq!(merged.member, "Aiden");
        assert_eq!(merged.web_token, "a".repeat(32), "the key must never be blanked by a form");
        assert_eq!(merged.coordinator, "mac-studio");

        // and the share form's fields still win when it posts them
        let patch = serde_json::json!({ "coordinator": "desk-32-a", "min_free_gb": 40 });
        let merged = merge_config(&merged, &patch).unwrap();
        assert_eq!(merged.coordinator, "desk-32-a");
        assert_eq!(merged.min_free_gb, 40);
        assert_eq!(merged.web_port, 9100);
    }

    // The dead-button check, from the other end: every name the dispatch table
    // advertises must actually be handled. A typo here used to mean a button
    // that spins forever with no error.
    #[test]
    fn dispatch_handles_every_advertised_command() {
        let (core, _share) = test_core("dispatch");
        for cmd in COMMANDS {
            // pick_repo opens a Finder folder-picker, and discover_coordinators
            // browses Bonjour for 2.5s — neither belongs in a unit test.
            if cmd == "pick_repo" || cmd == "discover_coordinators" {
                continue;
            }
            if let Err(e) = core.dispatch(cmd, &serde_json::json!({})) {
                assert!(!e.starts_with("unknown command"), "{} is not wired: {}", cmd, e);
            }
        }
        assert!(core.dispatch("nonsense", &serde_json::json!({})).unwrap_err().starts_with("unknown command"));
    }

    // Queue -> reorder -> variants -> enqueue, driven the way the browser does
    // it. This is the whole board loop through the real command surface.
    #[test]
    fn the_board_loop_works_through_dispatch() {
        let (core, _share) = test_core("boardloop");
        for i in 0..3 {
            core.dispatch("enqueue_job", &serde_json::json!({
                "id": format!("clip{}", i), "prompt": "storm over a roof", "width": 1080, "height": 1920
            })).expect("enqueue");
        }
        let b = core.dispatch("get_board", &serde_json::json!({})).unwrap();
        let queued = b["board"]["queued"].as_array().unwrap().clone();
        assert_eq!(queued.len(), 3);

        // drag the last card to the front
        let order: Vec<String> = vec![
            queued[2]["file"].as_str().unwrap().to_string(),
            queued[0]["file"].as_str().unwrap().to_string(),
            queued[1]["file"].as_str().unwrap().to_string(),
        ];
        core.dispatch("job_action", &serde_json::json!({"action":"reorder","order":order})).expect("reorder");
        let b = core.dispatch("get_board", &serde_json::json!({})).unwrap();
        assert_eq!(b["board"]["queued"][0]["id"], "clip2");

        // bump it to the priority lane
        let top = b["board"]["queued"][0]["file"].as_str().unwrap().to_string();
        core.dispatch("job_action", &serde_json::json!({"action":"promote","file":top})).expect("promote");
        let b = core.dispatch("get_board", &serde_json::json!({})).unwrap();
        assert_eq!(b["board"]["queued"][0]["priority"], "high");

        // ask for variants of it and queue the square one
        let top = b["board"]["queued"][0]["file"].as_str().unwrap().to_string();
        let v = core.dispatch("job_variants", &serde_json::json!({"lane":"queued","file":top})).expect("variants");
        let list = v["variants"].as_array().unwrap();
        let square = list
            .iter()
            .find(|x| x["job"]["width"] == 1080 && x["job"]["height"] == 1080)
            .expect("a square variant is offered");
        core.dispatch("enqueue_job", &serde_json::json!({"job": square["job"]})).expect("queue the variant");
        let b = core.dispatch("get_board", &serde_json::json!({})).unwrap();
        assert_eq!(b["board"]["queued"].as_array().unwrap().len(), 4);
        assert!(b["board"]["queued"].as_array().unwrap().iter().any(|c| c["aspect"] == "1:1"));
    }

    // The Team view has to show this Mac even before anyone else joins.
    #[test]
    fn presence_puts_this_mac_in_the_team_view() {
        let (core, share) = test_core("presence");
        publish_presence(&core, 1);
        assert!(share.join("presence").read_dir().unwrap().count() > 0, "a presence file is written");
        let m = core.dispatch("get_members", &serde_json::json!({})).unwrap();
        let list = m["members"].as_array().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["member"], "Test Person");
        assert_eq!(list[0]["is_you"], true);
        assert_eq!(list[0]["host"], this_host());
    }

    // End to end over a real socket: the gateway serves the same UI file, the
    // command surface answers, and a path outside the share is refused.
    #[test]
    fn the_gateway_serves_the_ui_and_the_api() {
        use std::io::{Read as _, Write as _};
        let (core, share) = test_core("gateway");
        std::fs::write(share.join("done/clip.mp4"), b"not really a video").unwrap();
        let g = web::start(core.clone(), 8901, false, "0".repeat(32)).expect("gateway binds");

        let req = |raw: String| -> String {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", g.port)).expect("connect");
            s.write_all(raw.as_bytes()).unwrap();
            let mut out = String::new();
            let _ = s.read_to_string(&mut out);
            out
        };
        let get = |path: &str| {
            req(format!("GET {} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n", path))
        };

        let page = get("/");
        assert!(page.starts_with("HTTP/1.1 200"), "{}", &page[..page.len().min(60)]);
        assert!(page.contains("LTX Mac Farm"), "it serves the popover's own page");

        let body = serde_json::json!({"cmd":"get_state","args":{}}).to_string();
        let res = req(format!(
            "POST /api/invoke HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(), body
        ));
        assert!(res.contains("\"ok\":true"), "{}", res);
        assert!(res.contains("counts"), "{}", res);

        // a file inside the share is served; anything else is not
        let ok = get(&format!("/file?path={}", share.join("done/clip.mp4").to_string_lossy()));
        assert!(ok.starts_with("HTTP/1.1 200"), "{}", &ok[..ok.len().min(60)]);
        assert!(ok.contains("video/mp4"));
        let denied = get("/file?path=/etc/passwd");
        assert!(denied.starts_with("HTTP/1.1 404"), "{}", &denied[..denied.len().min(80)]);
        assert!(get("/healthz").contains("\"ok\":true"));
        assert!(get("/nope").starts_with("HTTP/1.1 404"));

        g.stop();
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
