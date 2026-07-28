// LTX Mac Farm — the pipeline as data: board, reordering, variants, people.
//
// The share IS the database. Every fact here is derived from filenames and job
// files on the share, never from a cache, because five Macs write to it at once
// and any second source of truth would immediately disagree with the folder.
//
// Naming conventions, all set by farm_worker.sh / enqueue.sh — this module only
// reads and renames, it never invents a new scheme:
//
//   queue/<stamp>__<id>.job                     waiting, normal lane
//   queue/hi/<stamp>__<id>.job                  waiting, priority lane (claimed first)
//   queue/OOMRETRY_<stamp>__<id>.job            requeued after a memory kill
//   running/<stamp>__<id>.job.<HOST>.<pid>      claimed by HOST
//   running/<...>.heartbeat                     touched every 30s while rendering
//   running/.worker.<HOST>.info                 that Mac's live memory state
//   done/<...>.job.<HOST>.<pid>.ok              finished
//   done/<ID>.mp4  done/<ID>.json               the render + its metadata sidecar
//   done/proofs/<ID>_seed<N>.png                test-mode proof still
//   failed/<...>.job.<HOST>.<pid>.rc<N>         failed with exit code N
//
// ORDERING. Workers claim with `for cand in queue/hi/*.job queue/*.job` — a
// bash glob, so claim order is byte order on the filename, and the stamp is
// the first thing in it. That makes reordering a rename, not a database write:
// see reorder_queue().

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{home, list_dir_all, mtime_age, parse_host, parse_id, sh, this_host};

// Reordered jobs are stamped with a date that cannot occur, so they always sort
// ahead of anything enqueue.sh writes (which starts with the current year).
const TOP_STAMP: &str = "00000000_000000";

// ---------------------------------------------------------------------------
// Job files
// ---------------------------------------------------------------------------

/// Parse the KEY="value" body of a .job file (workers `source` it, so it's
/// plain shell). Tolerant on purpose: unknown keys, comments and blank lines
/// are skipped rather than failing the whole card.
pub fn parse_job_kv(text: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let Some((k, v)) = t.split_once('=') else { continue };
        let key = k.trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let mut val = v.trim().to_string();
        if val.len() >= 2 && val.starts_with('"') && val.ends_with('"') {
            val = val[1..val.len() - 1].to_string();
        }
        // undo the escaping enqueue.sh applies to prompts
        let val = val.replace("\\\"", "\"").replace("\\\\", "\\");
        m.insert(key.to_string(), val);
    }
    m
}

fn num<T: std::str::FromStr>(m: &HashMap<String, String>, k: &str, d: T) -> T {
    m.get(k).and_then(|v| v.trim().parse::<T>().ok()).unwrap_or(d)
}

fn text(m: &HashMap<String, String>, k: &str) -> String {
    m.get(k).cloned().unwrap_or_default()
}

/// One card on the board. Everything the UI needs to render a job in any lane,
/// flattened — the browser should never have to know the filename grammar.
#[derive(Serialize, Clone, Default)]
pub struct JobCard {
    pub file: String, // filename inside its lane folder, as it exists right now
    pub path: String, // absolute path, so the UI can open/reveal it
    pub lane: String, // queued | running | done | failed
    pub id: String,
    pub stamp: String,
    pub priority: String, // high | normal
    pub position: usize,  // 1-based claim order (queued lane only)
    pub prompt: String,
    pub kind: String, // TYPE: t2v | i2v | lora_i2v
    pub mode: String, // hero | test
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub seed: i64,
    pub fps: u32,
    pub perf: String,
    pub lora: String,
    pub image: String,
    pub min_ram_gb: u64,
    pub oom_retry: u32,
    pub host: String,     // which Mac claimed / ran it
    pub age_secs: u64,    // since the file last changed (heartbeat for running)
    pub mp4: String,      // done: the finished render
    pub mp4_mb: u64,
    pub proof: String,    // test mode: the proof still
    pub log: String,      // render log, if the worker kept one
    pub rc: String,       // failed: exit code
    pub duration_secs: u64,
    pub peak_mem_gb: f64,
    pub aspect: String,   // "9:16" etc — the board groups by shape
    pub member: String,   // who queued it (written by this app, absent from CLI jobs)
    pub run: String,       // which batch it belongs to, e.g. an overnight run
    pub retry: u32,        // how many times autopilot has re-run it
    pub review: String,    // "" | approved | retake  (from reviews/<ID>.json)
    pub review_by: String,
    pub review_note: String,
    pub est_secs: u64,     // how long this size usually takes on THIS farm
    pub eta_secs: u64,     // queued: when it should start; running: time left
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a.max(1) } else { gcd(b, a % b) }
}

fn aspect(w: u32, h: u32) -> String {
    if w == 0 || h == 0 {
        return String::new();
    }
    let g = gcd(w, h);
    format!("{}:{}", w / g, h / g)
}

// "<stamp>__<id>.job…" -> "<stamp>"
fn parse_stamp(name: &str) -> String {
    name.split("__").next().unwrap_or("").trim_start_matches("OOMRETRY_").to_string()
}

fn read_card(dir: &Path, file: &str, lane: &str) -> JobCard {
    let path = dir.join(file);
    let kv = std::fs::read_to_string(&path).map(|s| parse_job_kv(&s)).unwrap_or_default();
    let width = num(&kv, "WIDTH", 1080u32);
    let height = num(&kv, "HEIGHT", 1920u32);
    JobCard {
        file: file.to_string(),
        path: path.to_string_lossy().to_string(),
        lane: lane.to_string(),
        id: if kv.contains_key("ID") && !text(&kv, "ID").is_empty() {
            text(&kv, "ID")
        } else {
            parse_id(file)
        },
        stamp: parse_stamp(file),
        priority: "normal".into(),
        prompt: text(&kv, "PROMPT"),
        kind: {
            let t = text(&kv, "TYPE");
            if t.is_empty() { "t2v".into() } else { t }
        },
        mode: {
            let m = text(&kv, "MODE");
            if m.is_empty() { "hero".into() } else { m }
        },
        width,
        height,
        frames: num(&kv, "FRAMES", 97u32),
        seed: num(&kv, "SEED", 42i64),
        fps: num(&kv, "FPS", 24u32),
        perf: text(&kv, "PERF"),
        lora: text(&kv, "LORA"),
        image: text(&kv, "IMAGE"),
        min_ram_gb: num(&kv, "MIN_RAM_GB", 0u64),
        oom_retry: num(&kv, "OOM_RETRY", 0u32),
        member: text(&kv, "MEMBER"),
        run: text(&kv, "RUN"),
        retry: num(&kv, "RETRY", 0u32),
        age_secs: mtime_age(&path),
        aspect: aspect(width, height),
        ..Default::default()
    }
}

// The sidecar the worker writes next to a finished mp4. Only the few fields the
// board shows — a partial parse beats failing the card because a key moved.
fn merge_sidecar(card: &mut JobCard, done: &Path) {
    let side = done.join(format!("{}.json", card.id));
    let Ok(body) = std::fs::read_to_string(side) else { return };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else { return };
    if let Some(d) = v.get("duration_secs").and_then(|x| x.as_u64()) {
        card.duration_secs = d;
    }
    if let Some(p) = v.get("peak_mem_gb").and_then(|x| x.as_f64()) {
        card.peak_mem_gb = p;
    }
    if card.prompt.is_empty() {
        if let Some(p) = v.get("prompt").and_then(|x| x.as_str()) {
            card.prompt = p.to_string();
        }
    }
}

fn file_mb(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len() / 1_048_576).unwrap_or(0)
}

/// Every lane of the pipeline, in the order the farm will actually work through
/// it. `queued` is claim order: the hi/ lane first, then the normal queue,
/// each sorted the way the worker's glob sorts them.
#[derive(Serialize, Default)]
pub struct Board {
    pub root: String,
    pub queued: Vec<JobCard>,
    pub running: Vec<JobCard>,
    pub done: Vec<JobCard>,
    pub failed: Vec<JobCard>,
    pub totals: HashMap<String, usize>,
    pub reachable: bool,
}

pub fn board(root: &str, done_limit: usize) -> Board {
    let rootp = Path::new(root);
    let mut b = Board { root: root.to_string(), reachable: rootp.is_dir(), ..Default::default() };
    if !b.reachable {
        return b;
    }

    // --- queued: priority lane first, then the normal one -------------------
    let mut pos = 0usize;
    for (sub, prio) in [("queue/hi", "high"), ("queue", "normal")] {
        let dir = rootp.join(sub);
        let mut names: Vec<String> =
            list_dir_all(&dir).into_iter().filter(|n| n.ends_with(".job")).collect();
        names.sort(); // same byte order the worker's glob uses
        for n in names {
            let mut c = read_card(&dir, &n, "queued");
            c.priority = prio.into();
            pos += 1;
            c.position = pos;
            b.queued.push(c);
        }
    }

    // --- running: the job file plus its heartbeat --------------------------
    let running = rootp.join("running");
    let names = list_dir_all(&running);
    for n in names.iter().filter(|n| n.contains(".job.") && !n.ends_with(".heartbeat")) {
        let mut c = read_card(&running, n, "running");
        c.host = parse_host(n);
        // The heartbeat is the honest clock: the job file's mtime is when the
        // worker claimed it, the heartbeat is 30s ago if it's still alive.
        let hb = running.join(format!("{}.heartbeat", n));
        if hb.exists() {
            c.age_secs = mtime_age(&hb);
        }
        b.running.push(c);
    }
    b.running.sort_by(|a, z| a.age_secs.cmp(&z.age_secs));

    // --- done: newest first, with the mp4 + sidecar attached ---------------
    let done = rootp.join("done");
    let mut ok: Vec<String> = list_dir_all(&done).into_iter().filter(|n| n.ends_with(".ok")).collect();
    ok.sort_by_key(|n| mtime_age(&done.join(n)));
    for n in ok.into_iter().take(done_limit.max(1)) {
        let mut c = read_card(&done, &n, "done");
        c.host = parse_host(&n);
        let mp4 = done.join(format!("{}.mp4", c.id));
        if mp4.is_file() {
            c.mp4_mb = file_mb(&mp4);
            c.mp4 = mp4.to_string_lossy().to_string();
        }
        let proof = done.join("proofs").join(format!("{}_seed{}.png", c.id, c.seed));
        if proof.is_file() {
            c.proof = proof.to_string_lossy().to_string();
        }
        merge_sidecar(&mut c, &done);
        attach_log(&mut c, rootp);
        if let Some(r) = read_review(root, &c.id) {
            c.review = r.state;
            c.review_by = r.by;
            c.review_note = r.note;
        }
        b.done.push(c);
    }

    // --- failed: newest first, exit code pulled off the suffix -------------
    let failed = rootp.join("failed");
    // `retried_…` records are superseded: autopilot already put a fresh copy of
    // that job in the queue, so showing (and re-retrying) the old one would both
    // clutter the lane and loop forever.
    let mut bad: Vec<String> = list_dir_all(&failed)
        .into_iter()
        .filter(|n| n.contains(".rc") && !n.starts_with("retried_"))
        .collect();
    bad.sort_by_key(|n| mtime_age(&failed.join(n)));
    for n in bad {
        let mut c = read_card(&failed, &n, "failed");
        c.host = parse_host(&n);
        c.rc = n.rsplit(".rc").next().unwrap_or("").to_string();
        attach_log(&mut c, rootp);
        // A "needs another take" decision follows the clip id, not the lane —
        // autopilot reads it to know a human has already claimed this one.
        if let Some(r) = read_review(root, &c.id) {
            c.review = r.state;
            c.review_by = r.by;
            c.review_note = r.note;
        }
        b.failed.push(c);
    }

    b.totals.insert("queued".into(), b.queued.len());
    b.totals.insert("running".into(), b.running.len());
    b.totals.insert("done".into(), b.done.len());
    b.totals.insert("failed".into(), b.failed.len());
    b
}

fn attach_log(card: &mut JobCard, root: &Path) {
    if card.host.is_empty() || card.id.is_empty() {
        return;
    }
    let p = root.join("logs").join(format!("{}.{}.log", card.id, card.host));
    if p.is_file() {
        card.log = p.to_string_lossy().to_string();
    }
}

// ---------------------------------------------------------------------------
// Moving jobs around
// ---------------------------------------------------------------------------

// Only ever touch a name that came out of board() — never a caller-supplied
// path. A file name with a slash or .. in it would let the web gateway rename
// things outside the share.
fn safe_file(name: &str) -> Result<&str, String> {
    if name.is_empty() || name.contains('/') || name.contains("..") {
        return Err(format!("not a job file name: {}", name));
    }
    Ok(name)
}

fn find_queued(root: &str, file: &str) -> Option<PathBuf> {
    let f = safe_file(file).ok()?;
    for sub in ["queue", "queue/hi"] {
        let p = Path::new(root).join(sub).join(f);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Put a waiting job in the priority lane (or take it back out). A rename, so
/// it's atomic — a worker either claims the old path or the new one, never both.
pub fn set_priority(root: &str, file: &str, high: bool) -> Result<String, String> {
    let from = find_queued(root, file).ok_or_else(|| format!("{} is no longer waiting — a Mac may have claimed it", file))?;
    let dir = if high { Path::new(root).join("queue/hi") } else { Path::new(root).join("queue") };
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let to = dir.join(file);
    if from == to {
        return Ok(format!("{} is already in the {} lane", file, if high { "priority" } else { "normal" }));
    }
    std::fs::rename(&from, &to).map_err(|e| e.to_string())?;
    Ok(if high {
        "Bumped to the priority lane — the next free Mac takes it.".to_string()
    } else {
        "Back in the normal queue.".to_string()
    })
}

/// Apply an explicit order to the waiting jobs. `order` is the file names as
/// the board showed them, in the order the user dragged them into.
///
/// Renaming is the whole mechanism: the worker claims by glob order, so the
/// stamp is the priority. Everything reordered gets the impossible TOP_STAMP
/// date plus a sequence number, which keeps hand-ordered work ahead of jobs
/// enqueued later while preserving the order inside the group.
pub fn reorder_queue(root: &str, order: &[String]) -> Result<String, String> {
    let mut moved = 0usize;
    for (i, file) in order.iter().enumerate() {
        let Some(from) = find_queued(root, file) else { continue };
        let id = parse_id(file);
        let lane = from.parent().ok_or("no parent dir")?.to_path_buf();
        let want = format!("{}_{:04}__{}.job", TOP_STAMP, i + 1, id);
        let to = lane.join(&want);
        if from == to {
            continue;
        }
        // A claim can land between the listing and the rename. Skip, don't fail
        // the whole reorder: the rest of the order is still worth applying.
        if std::fs::rename(&from, &to).is_ok() {
            moved += 1;
        }
    }
    if moved == 0 {
        return Err("Nothing was reordered — those jobs have already been claimed.".into());
    }
    Ok(format!("Reordered {} job(s). Workers claim top-down.", moved))
}

/// Drop a waiting job. Claimed jobs are left alone: killing a render from here
/// would leave a half-written mp4 and a worker that thinks it still owns the file.
pub fn cancel_job(root: &str, file: &str) -> Result<String, String> {
    let p = find_queued(root, file)
        .ok_or_else(|| format!("{} isn't waiting any more — it's been claimed, so stop it on that Mac.", file))?;
    std::fs::remove_file(&p).map_err(|e| e.to_string())?;
    Ok(format!("Removed {} from the queue.", parse_id(file)))
}

/// Send a failed job back to the queue, or re-run a finished one. Strips the
/// worker's claim suffix (`.job.<HOST>.<pid>[.ok|.rcN]`) so it's a plain .job
/// again, and re-stamps it so it goes to the back of the line.
pub fn requeue_job(root: &str, lane: &str, file: &str, stamp: &str) -> Result<String, String> {
    let f = safe_file(file)?;
    let dir = match lane {
        "failed" => "failed",
        "done" => "done",
        other => return Err(format!("can't requeue from {}", other)),
    };
    let from = Path::new(root).join(dir).join(f);
    if !from.is_file() {
        return Err(format!("{} not found in {}/", f, dir));
    }
    let id = parse_id(f);
    let queue = Path::new(root).join("queue");
    std::fs::create_dir_all(&queue).map_err(|e| e.to_string())?;
    let to = queue.join(format!("{}__{}.job", stamp, id));

    if dir == "failed" {
        std::fs::rename(&from, &to).map_err(|e| e.to_string())?;
        Ok(format!("{} is back in the queue.", id))
    } else {
        // A finished job stays finished — re-running copies it so the record of
        // the first render (and its mp4) survives.
        std::fs::copy(&from, &to).map_err(|e| e.to_string())?;
        Ok(format!("Queued another pass of {}.", id))
    }
}

// ---------------------------------------------------------------------------
// Writing new jobs — enqueue.sh's format, without the shell
// ---------------------------------------------------------------------------

/// What the Board's "queue a clip" form posts. Field-for-field the same job
/// file enqueue.sh writes, so a job created in the browser is indistinguishable
/// from one created in Terminal.
#[derive(Deserialize, Clone)]
#[serde(default)]
pub struct NewJob {
    pub id: String,
    pub prompt: String,
    pub kind: String, // t2v | i2v | lora_i2v
    pub mode: String, // hero | test
    pub image: String,
    pub lora: String,
    pub lora_scale: String,
    pub still_prompt: String,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub seed: i64,
    pub fps: u32,
    pub perf: String,
    pub extra: String,
    pub priority: String, // high | normal
    pub min_ram_gb: u64,
    pub sweep: u32, // >1 = that many seeds, split across the farm
    pub member: String, // who asked for it
    pub run: String,    // batch name, e.g. "overnight_2026-07-28"
    pub retry: u32,     // set by autopilot when it re-runs a failure
}

impl Default for NewJob {
    fn default() -> Self {
        Self {
            id: String::new(),
            prompt: String::new(),
            kind: "t2v".into(),
            mode: "hero".into(),
            image: String::new(),
            lora: String::new(),
            lora_scale: "1.0".into(),
            still_prompt: String::new(),
            width: 1080,
            height: 1920,
            frames: 97,
            seed: 42,
            fps: 24,
            perf: String::new(),
            extra: String::new(),
            priority: "normal".into(),
            min_ram_gb: 0,
            sweep: 0,
            member: String::new(),
            run: String::new(),
            retry: 0,
        }
    }
}

// IDs become filenames and later an mp4 name, so keep them to something a
// shell, SMB and Finder all agree on.
pub fn safe_id(s: &str) -> String {
    let cleaned: String = s
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let cleaned = cleaned.trim_matches('_').to_string();
    if cleaned.is_empty() { "job".into() } else { cleaned.chars().take(60).collect() }
}

// A .job file is sourced by bash. A prompt containing " or \ or $ must not be
// able to become a command, so quote-escape it and refuse the rest.
fn job_value(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`")
        .replace(['\n', '\r'], " ")
}

// FRAMES must be 8k+1 or the model rejects it — snap instead of erroring, so a
// slider in the browser can't produce an unrenderable job.
pub fn snap_frames(f: u32) -> u32 {
    let f = f.clamp(9, 481);
    ((f.saturating_sub(1)) / 8) * 8 + 1
}

fn job_body(j: &NewJob, seed: i64) -> String {
    let mut s = String::new();
    s.push_str(&format!("ID=\"{}\"\n", safe_id(&j.id)));
    s.push_str(&format!("TYPE=\"{}\"\n", job_value(&j.kind)));
    s.push_str(&format!("PROMPT=\"{}\"\n", job_value(&j.prompt)));
    s.push_str(&format!("IMAGE=\"{}\"\n", job_value(&j.image)));
    s.push_str(&format!("LORA=\"{}\"\n", job_value(&j.lora)));
    s.push_str(&format!("LORA_SCALE={}\n", if j.lora_scale.trim().is_empty() { "1.0".into() } else { job_value(&j.lora_scale) }));
    s.push_str(&format!("STILL_PROMPT=\"{}\"\n", job_value(&j.still_prompt)));
    s.push_str(&format!("WIDTH={}\n", j.width.clamp(256, 3840)));
    s.push_str(&format!("HEIGHT={}\n", j.height.clamp(256, 3840)));
    s.push_str(&format!("FRAMES={}\n", snap_frames(j.frames)));
    s.push_str(&format!("SEED={}\n", seed));
    s.push_str(&format!("FPS={}\n", j.fps.clamp(8, 60)));
    s.push_str(&format!("EXTRA=\"{}\"\n", job_value(&j.extra)));
    s.push_str(&format!("MODE=\"{}\"\n", if j.mode == "test" { "test" } else { "hero" }));
    if !j.perf.trim().is_empty() && j.perf != "auto" {
        s.push_str(&format!("PERF=\"{}\"\n", job_value(&j.perf)));
    }
    if j.min_ram_gb > 0 {
        s.push_str(&format!("MIN_RAM_GB={}\n", j.min_ram_gb));
    }
    // Extra keys the worker doesn't read (it sources the file, so unknown vars
    // are simply set and ignored) but the board does: who asked for this, which
    // batch it belongs to, and how many times autopilot has retried it.
    if !j.member.trim().is_empty() {
        s.push_str(&format!("MEMBER=\"{}\"\n", job_value(&j.member)));
    }
    if !j.run.trim().is_empty() {
        s.push_str(&format!("RUN=\"{}\"\n", job_value(&j.run)));
    }
    if j.retry > 0 {
        s.push_str(&format!("RETRY={}\n", j.retry));
    }
    s
}

/// Write one job (or a seed sweep) into the queue. Returns the file names, so
/// the UI can highlight what it just created.
pub fn enqueue(root: &str, j: &NewJob, stamp: &str) -> Result<Vec<String>, String> {
    if j.prompt.trim().is_empty() {
        return Err("A prompt is required — that's the one thing the render can't guess.".into());
    }
    let dir = if j.priority == "high" {
        Path::new(root).join("queue/hi")
    } else {
        Path::new(root).join("queue")
    };
    std::fs::create_dir_all(&dir).map_err(|e| {
        format!("Can't write to {} — {}. Is the share mounted?", dir.display(), e)
    })?;

    let base = safe_id(if j.id.trim().is_empty() { &j.prompt } else { &j.id });
    let mut written = Vec::new();
    let n = j.sweep.clamp(0, 24);
    let seeds: Vec<(String, i64)> = if n > 1 {
        (0..n).map(|i| (format!("{}_s{}", base, 1000 + i), 1000 + i as i64)).collect()
    } else {
        vec![(base.clone(), j.seed)]
    };

    for (i, (id, seed)) in seeds.iter().enumerate() {
        let mut jj = j.clone();
        jj.id = id.clone();
        let name = format!("{}_{:02}__{}.job", stamp, i, id);
        let p = dir.join(&name);
        std::fs::write(&p, job_body(&jj, *seed)).map_err(|e| format!("{}: {}", p.display(), e))?;
        written.push(name);
    }
    Ok(written)
}

// ---------------------------------------------------------------------------
// Variants — "give me the same shot, but…"
// ---------------------------------------------------------------------------

/// A suggested re-run of an existing job. The UI shows these as tick-boxes so
/// a whole set of sizes or prompt tweaks becomes one click and N jobs.
#[derive(Serialize, Clone)]
pub struct Variant {
    pub group: String, // size | prompt | seed | quality
    pub label: String,
    pub why: String,
    pub job: serde_json::Value, // a NewJob, ready to post back to enqueue
}

fn variant(group: &str, label: &str, why: &str, j: &NewJob) -> Variant {
    Variant {
        group: group.into(),
        label: label.into(),
        why: why.into(),
        job: serde_json::json!({
            "id": j.id, "prompt": j.prompt, "kind": j.kind, "mode": j.mode,
            "image": j.image, "lora": j.lora, "lora_scale": j.lora_scale,
            "still_prompt": j.still_prompt, "width": j.width, "height": j.height,
            "frames": j.frames, "seed": j.seed, "fps": j.fps, "perf": j.perf,
            "extra": j.extra, "priority": "normal", "min_ram_gb": j.min_ram_gb, "sweep": 0,
            "run": j.run
        }),
    }
}

fn base_from(card: &JobCard) -> NewJob {
    NewJob {
        id: card.id.clone(),
        prompt: card.prompt.clone(),
        kind: if card.kind.is_empty() { "t2v".into() } else { card.kind.clone() },
        mode: if card.mode.is_empty() { "hero".into() } else { card.mode.clone() },
        image: card.image.clone(),
        lora: card.lora.clone(),
        still_prompt: String::new(),
        width: if card.width == 0 { 1080 } else { card.width },
        height: if card.height == 0 { 1920 } else { card.height },
        frames: if card.frames == 0 { 97 } else { card.frames },
        seed: card.seed,
        fps: if card.fps == 0 { 24 } else { card.fps },
        min_ram_gb: card.min_ram_gb,
        run: card.run.clone(),
        ..Default::default()
    }
}

// Where each size actually gets used. Written as the deliverable, not the
// dimensions, because that's how the request arrives ("we need a square one").
const SIZES: [(&str, u32, u32, &str); 4] = [
    ("Vertical 9:16", 1080, 1920, "Reels, TikTok, Shorts"),
    ("Square 1:1", 1080, 1080, "Feed posts, LinkedIn"),
    ("Landscape 16:9", 1920, 1080, "YouTube, site hero, presentations"),
    ("Wide 4:5", 1080, 1350, "Instagram feed's tallest safe crop"),
];

// Prompt edits that change the look without changing the subject. Kept short
// and additive so they compose with whatever the original prompt said.
const TWEAKS: [(&str, &str); 5] = [
    ("golden hour light, warm rim light", "Warmer, sunnier grade"),
    ("overcast storm light, moody, desaturated", "Storm-damage mood — closer to the real job sites"),
    ("slow push in, shallow depth of field", "Adds camera movement instead of a static frame"),
    ("wide establishing shot", "Pulls back for context — good as an opener"),
    ("handheld documentary feel, natural light", "Reads as filmed rather than generated"),
];

/// Recommend re-runs for a card: the same shot at every delivery size, a few
/// prompt edits, a seed sweep, and a cheap proof still.
pub fn variants_for(card: &JobCard) -> Vec<Variant> {
    let base = base_from(card);
    let mut out = Vec::new();

    for (label, w, h, why) in SIZES {
        if w == base.width && h == base.height {
            continue; // it already exists in this shape
        }
        let mut j = base.clone();
        j.width = w;
        j.height = h;
        j.id = format!("{}_{}x{}", card.id, w, h);
        out.push(variant("size", label, why, &j));
    }

    for (add, why) in TWEAKS {
        let mut j = base.clone();
        let sep = if j.prompt.trim_end().ends_with(',') { " " } else { ", " };
        j.prompt = format!("{}{}{}", j.prompt.trim_end(), sep, add);
        j.id = format!("{}_{}", card.id, safe_id(add.split(',').next().unwrap_or(add)));
        out.push(variant("prompt", add, why, &j));
    }

    let mut sweep = base.clone();
    sweep.id = format!("{}_sweep", card.id);
    sweep.sweep = 4;
    let mut sw = variant("seed", "4-seed sweep", "Same prompt, four different takes — the farm splits them across Macs", &sweep);
    sw.job["sweep"] = serde_json::json!(4);
    out.push(sw);

    let mut proof = base.clone();
    proof.mode = "test".into();
    proof.id = format!("{}_proof", card.id);
    out.push(variant(
        "quality",
        "Cheap proof still",
        "Seconds instead of an hour — check the framing before committing the farm",
        &proof,
    ));

    out
}

/// Look a card up by lane + file so the UI can ask for variants of anything on
/// the board without shipping the whole card back to us.
pub fn find_card(root: &str, lane: &str, file: &str) -> Option<JobCard> {
    let b = board(root, 400);
    let list = match lane {
        "queued" => &b.queued,
        "running" => &b.running,
        "done" => &b.done,
        "failed" => &b.failed,
        _ => return None,
    };
    list.iter().find(|c| c.file == file).cloned()
}

// ---------------------------------------------------------------------------
// People — who is on the farm and what their Mac is doing
// ---------------------------------------------------------------------------

/// What one Mac publishes about itself. Written by every running copy of the
/// app to `presence/<host>.json` on the share; read by all of them. That's the
/// whole protocol — no server, no ports between Macs.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct Presence {
    pub host: String,
    pub member: String, // the person sitting at it
    pub model: String,  // hw.model, e.g. Mac16,10
    pub ram_gb: u64,
    pub role: String, // coordinator | worker
    pub perf: String,
    pub app_version: String,
    pub gateway: String, // this Mac's own web gateway, so the team can hop to it
    pub ts: u64,
}

pub fn presence_dir(root: &str) -> PathBuf {
    Path::new(root).join("presence")
}

pub fn write_presence(root: &str, p: &Presence) -> Result<(), String> {
    let dir = presence_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let body = serde_json::to_string_pretty(p).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{}.json", crate::safe_host(&p.host))), body).map_err(|e| e.to_string())
}

/// One row in the Team view: the person, their Mac, and what it's doing right now.
#[derive(Serialize, Clone, Default)]
pub struct Member {
    pub host: String,
    pub member: String,
    pub model: String,
    pub ram_gb: u64,
    pub role: String,
    pub perf: String,
    pub state: String, // rendering | idle | paused | backoff | offline
    pub detail: String,
    pub job: String,
    pub job_prompt: String,
    pub elapsed_secs: u64,
    pub last_seen_secs: u64,
    pub gateway: String,
    pub free_pct: u64,
    pub pressure: u64,
    pub swap_mb: u64,
    pub budget_gb: f64,
    pub done_count: usize,
    pub is_you: bool,
    pub app: bool,    // the menubar app is running there
    pub worker: bool, // farm_worker.sh is running there
}

// A Mac that hasn't updated its presence file in this long is treated as gone.
const OFFLINE_AFTER: u64 = 90;

/// Merge the three things that know about a Mac — the app's presence file, the
/// worker's .info file, and the live heartbeat in running/ — into one row.
///
/// Any of the three can be missing: someone can run the app without a worker
/// (a producer watching the board), or a worker without the app (a headless
/// Mac), and both should still appear.
pub fn members(root: &str) -> Vec<Member> {
    let rootp = Path::new(root);
    let me = this_host();
    let mut map: HashMap<String, Member> = HashMap::new();

    // 1. the app's own presence files
    for n in list_dir_all(&presence_dir(root)).iter().filter(|n| n.ends_with(".json")) {
        let path = presence_dir(root).join(n);
        let Ok(body) = std::fs::read_to_string(&path) else { continue };
        let Ok(p) = serde_json::from_str::<Presence>(&body) else { continue };
        if p.host.trim().is_empty() {
            continue;
        }
        let age = mtime_age(&path);
        let key = p.host.to_lowercase();
        map.insert(
            key,
            Member {
                host: p.host.clone(),
                member: p.member.clone(),
                model: p.model.clone(),
                ram_gb: p.ram_gb,
                role: p.role.clone(),
                perf: p.perf.clone(),
                state: if age > OFFLINE_AFTER { "offline".into() } else { "idle".into() },
                last_seen_secs: age,
                gateway: p.gateway.clone(),
                app: age <= OFFLINE_AFTER,
                is_you: p.host.eq_ignore_ascii_case(&me),
                ..Default::default()
            },
        );
    }

    // 2. the worker's published memory state
    let running = rootp.join("running");
    let names = list_dir_all(&running);
    for n in names.iter().filter(|n| n.starts_with(".worker.") && n.ends_with(".info")) {
        let path = running.join(n);
        let Ok(body) = std::fs::read_to_string(&path) else { continue };
        let kv = parse_job_kv(&body);
        let host = if text(&kv, "HOST").is_empty() {
            n.trim_start_matches(".worker.").trim_end_matches(".info").to_string()
        } else {
            text(&kv, "HOST")
        };
        let age = mtime_age(&path);
        let m = map.entry(host.to_lowercase()).or_insert_with(|| Member {
            host: host.clone(),
            is_you: host.eq_ignore_ascii_case(&me),
            ..Default::default()
        });
        m.host = host.clone();
        m.worker = age <= OFFLINE_AFTER;
        if m.ram_gb == 0 {
            m.ram_gb = num(&kv, "RAM_GB", 0u64);
        }
        if m.perf.is_empty() {
            m.perf = text(&kv, "PERF");
        }
        m.free_pct = num(&kv, "FREE_PCT", 0u64);
        m.pressure = num(&kv, "PRESSURE", 0u64);
        m.swap_mb = num(&kv, "SWAP_MB", 0u64);
        m.budget_gb = num(&kv, "BUDGET_GB", 0.0f64);
        let state = text(&kv, "STATE");
        if age <= OFFLINE_AFTER && !state.is_empty() {
            // STATE is "idle" | "rendering" | "paused:disk" | "backoff:oom"
            let (head, tail) = state.split_once(':').unwrap_or((state.as_str(), ""));
            m.state = head.to_string();
            m.detail = match tail {
                "disk" => "paused — low disk".into(),
                "memory" => "paused — low memory".into(),
                "oom" => "backing off after a memory kill".into(),
                _ => String::new(),
            };
        }
        m.last_seen_secs = m.last_seen_secs.min(age);
    }

    // 3. what it is rendering this second
    for n in names.iter().filter(|n| n.ends_with(".heartbeat")) {
        let age = mtime_age(&running.join(n));
        if age > 120 {
            continue; // stale — farm_status.sh --reap will requeue that job
        }
        let job_name = n.trim_end_matches(".heartbeat");
        let host = parse_host(job_name);
        let card = read_card(&running, job_name, "running");
        let m = map.entry(host.to_lowercase()).or_insert_with(|| Member {
            host: host.clone(),
            is_you: host.eq_ignore_ascii_case(&me),
            ..Default::default()
        });
        m.host = host.clone();
        m.state = "rendering".into();
        m.job = card.id.clone();
        m.job_prompt = card.prompt.clone();
        m.elapsed_secs = mtime_age(&running.join(job_name));
        m.last_seen_secs = m.last_seen_secs.min(age);
        m.worker = true;
    }

    // 4. how much each Mac has actually finished (bragging rights, and a quick
    //    read on whether one Mac is doing all the work)
    for n in list_dir_all(&rootp.join("done")).iter().filter(|n| n.ends_with(".ok")) {
        let host = parse_host(n);
        if let Some(m) = map.get_mut(&host.to_lowercase()) {
            m.done_count += 1;
        }
    }

    let mut v: Vec<Member> = map.into_values().collect();
    // you first, then whoever is rendering, then by name — the useful order
    v.sort_by(|a, b| {
        b.is_you
            .cmp(&a.is_you)
            .then_with(|| (a.state == "rendering").cmp(&(b.state == "rendering")).reverse())
            .then_with(|| a.host.to_lowercase().cmp(&b.host.to_lowercase()))
    });
    v
}

pub fn mac_model() -> String {
    let m = sh("sysctl -n hw.model").trim().to_string();
    if m.is_empty() { "Mac".into() } else { m }
}

pub fn ram_gb() -> u64 {
    sh("sysctl -n hw.memsize").trim().parse::<u64>().unwrap_or(0) / 1024 / 1024 / 1024
}

/// The person's name, defaulted from macOS's full-name field so the Team view
/// isn't a list of hostnames on day one.
pub fn default_member_name() -> String {
    let n = sh("id -F 2>/dev/null").trim().to_string();
    if !n.is_empty() {
        return n;
    }
    let u = sh("id -un").trim().to_string();
    if u.is_empty() { home().rsplit('/').next().unwrap_or("").to_string() } else { u }
}

// ---------------------------------------------------------------------------
// Review — the cherry-pick loop, as files on the share
// ---------------------------------------------------------------------------
//
// One small JSON per clip in <root>/reviews/. Not inside done/ because done/ is
// the worker's output folder and a person's opinion isn't output; keeping them
// apart means a `rm done/*` clean-up doesn't erase what the team decided.

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct Review {
    pub id: String,
    pub state: String, // approved | retake | "" (cleared)
    pub by: String,
    pub note: String,
    pub ts: u64,
}

pub fn reviews_dir(root: &str) -> PathBuf {
    Path::new(root).join("reviews")
}

pub fn read_review(root: &str, id: &str) -> Option<Review> {
    let p = reviews_dir(root).join(format!("{}.json", safe_id(id)));
    let body = std::fs::read_to_string(p).ok()?;
    serde_json::from_str(&body).ok()
}

pub fn write_review(root: &str, r: &Review) -> Result<String, String> {
    if r.id.trim().is_empty() {
        return Err("no clip named".into());
    }
    let dir = reviews_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let p = dir.join(format!("{}.json", safe_id(&r.id)));
    if r.state.trim().is_empty() {
        let _ = std::fs::remove_file(&p);
        return Ok(format!("Cleared the review on {}.", r.id));
    }
    if r.state != "approved" && r.state != "retake" {
        return Err(format!("unknown review state: {}", r.state));
    }
    let body = serde_json::to_string_pretty(r).map_err(|e| e.to_string())?;
    std::fs::write(&p, body).map_err(|e| e.to_string())?;
    Ok(match r.state.as_str() {
        "approved" => format!("{} approved.", r.id),
        _ => format!("{} marked for another take.", r.id),
    })
}

// ---------------------------------------------------------------------------
// Proof stills — what the test mode leaves behind
// ---------------------------------------------------------------------------

/// One cherry-pickable still. Test-mode jobs render these in seconds; the whole
/// point is to pick winners before spending an hour of farm time each.
#[derive(Serialize, Clone)]
pub struct Proof {
    pub id: String,       // the job id the still came from
    pub path: String,     // the png on the share
    pub seed: i64,
    pub age_secs: u64,
    pub review: String,
    pub prompt: String,
    pub width: u32,
    pub height: u32,
    pub done_file: String, // the .ok record, so "render hero" can find its job
    pub rendered: bool,    // a hero version of this id already exists
}

/// Every proof still on the share, newest first, with its prompt recovered from
/// the finished job record and whether a hero render already exists.
pub fn proofs(root: &str, limit: usize) -> Vec<Proof> {
    let rootp = Path::new(root);
    let done = rootp.join("done");
    let dir = done.join("proofs");
    let mut names: Vec<String> =
        list_dir_all(&dir).into_iter().filter(|n| n.ends_with(".png")).collect();
    names.sort_by_key(|n| mtime_age(&dir.join(n)));

    // done/*.ok records, so a proof can show its prompt and be promoted to hero
    let cards: Vec<JobCard> = board(root, 400).done;
    let heroes: HashSet<String> = cards
        .iter()
        .filter(|c| c.mode != "test")
        .map(|c| c.id.clone())
        .collect();

    names
        .into_iter()
        .take(limit.max(1))
        .map(|n| {
            // "<ID>_seed<N>.png"
            let stem = n.trim_end_matches(".png");
            let (id, seed) = match stem.rsplit_once("_seed") {
                Some((i, s)) => (i.to_string(), s.parse::<i64>().unwrap_or(0)),
                None => (stem.to_string(), 0),
            };
            let card = cards.iter().find(|c| c.id == id);
            Proof {
                path: dir.join(&n).to_string_lossy().to_string(),
                seed,
                age_secs: mtime_age(&dir.join(&n)),
                review: read_review(root, &id).map(|r| r.state).unwrap_or_default(),
                prompt: card.map(|c| c.prompt.clone()).unwrap_or_default(),
                width: card.map(|c| c.width).unwrap_or(1080),
                height: card.map(|c| c.height).unwrap_or(1920),
                done_file: card.map(|c| c.file.clone()).unwrap_or_default(),
                rendered: heroes.contains(&id),
                id,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Assets and LoRAs — what an image-to-video job can point at
// ---------------------------------------------------------------------------

const IMAGE_EXT: [&str; 5] = ["png", "jpg", "jpeg", "webp", "heic"];

pub fn list_assets(root: &str) -> serde_json::Value {
    let is = |n: &String, exts: &[&str]| {
        n.rsplit_once('.')
            .map(|(_, e)| exts.contains(&e.to_lowercase().as_str()))
            .unwrap_or(false)
    };
    let mut images: Vec<String> = list_dir_all(&Path::new(root).join("assets"))
        .into_iter()
        .filter(|n| !n.starts_with('.') && is(n, &IMAGE_EXT))
        .collect();
    images.sort();
    let mut loras: Vec<String> = list_dir_all(&Path::new(root).join("loras"))
        .into_iter()
        .filter(|n| n.ends_with(".safetensors"))
        .collect();
    loras.sort();
    serde_json::json!({ "images": images, "loras": loras })
}

/// Where an uploaded still is allowed to land, with a name that can't escape
/// the assets folder or arrive as a shell surprise.
pub fn asset_target(root: &str, name: &str) -> Result<PathBuf, String> {
    let base = name.rsplit('/').next().unwrap_or(name);
    let (stem, ext) = base.rsplit_once('.').ok_or("the file needs an extension")?;
    let ext = ext.to_lowercase();
    if !IMAGE_EXT.contains(&ext.as_str()) {
        return Err(format!("{} isn't an image the farm can use", ext));
    }
    let stem = safe_id(stem);
    let dir = Path::new(root).join("assets");
    std::fs::create_dir_all(&dir).map_err(|e| format!("can't write to assets/: {}", e))?;
    Ok(dir.join(format!("{}.{}", stem, ext)))
}

// ---------------------------------------------------------------------------
// Stats and estimates — from the sidecars the worker already writes
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Default)]
pub struct HostStat {
    pub host: String,
    pub clips: usize,
    pub secs: u64,
    pub avg_secs: u64,
    pub clips_24h: usize,
    pub peak_mem_gb: f64,
    pub budget_gb: f64,
}

#[derive(Serialize, Clone, Default)]
pub struct SizeStat {
    pub label: String, // "1080×1920 · 97f · hero"
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub mode: String,
    pub clips: usize,
    pub avg_secs: u64,
}

#[derive(Serialize, Clone, Default)]
pub struct Stats {
    pub clips: usize,
    pub clips_24h: usize,
    pub secs_24h: u64,
    pub avg_secs: u64,
    pub per_host: Vec<HostStat>,
    pub by_size: Vec<SizeStat>,
    pub over_budget: usize, // renders that peaked above the Mac's own budget
    pub sample: usize,      // how many sidecars this is based on
}

// The worker's sidecar, as far as the stats care.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Sidecar {
    id: String,
    mode: String,
    width: u32,
    height: u32,
    frames: u32,
    worker: String,
    duration_secs: u64,
    peak_mem_gb: f64,
    budget_gb: f64,
}

fn read_sidecars(root: &str) -> Vec<(Sidecar, u64)> {
    let done = Path::new(root).join("done");
    list_dir_all(&done)
        .into_iter()
        .filter(|n| n.ends_with(".json") && !n.starts_with('.'))
        .filter_map(|n| {
            let p = done.join(&n);
            let body = std::fs::read_to_string(&p).ok()?;
            let s: Sidecar = serde_json::from_str(&body).ok()?;
            if s.duration_secs == 0 {
                return None; // a test-mode still or an interrupted write
            }
            Some((s, mtime_age(&p)))
        })
        .collect()
}

fn size_key(w: u32, h: u32, f: u32, mode: &str) -> String {
    format!("{}x{}x{}x{}", w, h, f, mode)
}

pub fn stats(root: &str) -> Stats {
    let cards = read_sidecars(root);
    let mut st = Stats { sample: cards.len(), ..Default::default() };
    if cards.is_empty() {
        return st;
    }
    let day = 24 * 3600;
    let mut hosts: HashMap<String, HostStat> = HashMap::new();
    let mut sizes: HashMap<String, SizeStat> = HashMap::new();
    let mut total = 0u64;

    for (s, age) in &cards {
        total += s.duration_secs;
        st.clips += 1;
        if *age <= day {
            st.clips_24h += 1;
            st.secs_24h += s.duration_secs;
        }
        let h = hosts.entry(s.worker.to_lowercase()).or_insert_with(|| HostStat {
            host: if s.worker.is_empty() { "?".into() } else { s.worker.clone() },
            ..Default::default()
        });
        h.clips += 1;
        h.secs += s.duration_secs;
        if *age <= day {
            h.clips_24h += 1;
        }
        h.peak_mem_gb = h.peak_mem_gb.max(s.peak_mem_gb);
        h.budget_gb = h.budget_gb.max(s.budget_gb);

        let key = size_key(s.width, s.height, s.frames, &s.mode);
        let z = sizes.entry(key).or_insert_with(|| SizeStat {
            label: format!(
                "{}×{} · {}f · {}",
                s.width,
                s.height,
                s.frames,
                if s.mode.is_empty() { "hero" } else { &s.mode }
            ),
            width: s.width,
            height: s.height,
            frames: s.frames,
            mode: s.mode.clone(),
            ..Default::default()
        });
        z.clips += 1;
        z.avg_secs += s.duration_secs; // summed here, averaged below

        if s.budget_gb > 0.0 && s.peak_mem_gb > s.budget_gb {
            st.over_budget += 1;
        }
    }

    st.avg_secs = total / st.clips.max(1) as u64;
    let mut per_host: Vec<HostStat> = hosts
        .into_values()
        .map(|mut h| {
            h.avg_secs = h.secs / h.clips.max(1) as u64;
            h
        })
        .collect();
    per_host.sort_by(|a, b| b.clips.cmp(&a.clips));
    st.per_host = per_host;

    let mut by_size: Vec<SizeStat> = sizes
        .into_values()
        .map(|mut z| {
            z.avg_secs /= z.clips.max(1) as u64;
            z
        })
        .collect();
    by_size.sort_by(|a, b| b.clips.cmp(&a.clips));
    st.by_size = by_size;
    st
}

// A last-resort guess when the farm has never rendered anything: a 1080×1920
// hero at 97 frames on an M4 takes roughly half an hour. Deliberately rough —
// it's replaced by real history the moment one clip finishes.
const FALLBACK_HERO_SECS: u64 = 1800;
const FALLBACK_TEST_SECS: u64 = 90;

/// How long this shape of job usually takes on THIS farm. Exact match first,
/// then scaled from the average by pixel-frames, then the fallback.
pub fn estimate_secs(st: &Stats, w: u32, h: u32, frames: u32, mode: &str) -> u64 {
    let mode = if mode.is_empty() { "hero" } else { mode };
    if mode == "test" {
        // a proof still doesn't render video at all
        let exact = st.by_size.iter().find(|z| z.mode == "test");
        return exact.map(|z| z.avg_secs).unwrap_or(FALLBACK_TEST_SECS);
    }
    if let Some(z) = st
        .by_size
        .iter()
        .find(|z| z.width == w && z.height == h && z.frames == frames && z.mode == mode)
    {
        return z.avg_secs.max(1);
    }
    let want = (w as u64) * (h as u64) * (frames.max(1) as u64);
    // scale off the closest thing we HAVE measured, not off the global average:
    // a 49-frame 720p job and a 97-frame 1080p job aren't the same work.
    if let Some(z) = st.by_size.iter().filter(|z| z.mode != "test").max_by_key(|z| z.clips) {
        let have = (z.width as u64) * (z.height as u64) * (z.frames.max(1) as u64);
        if have > 0 && z.avg_secs > 0 {
            return ((z.avg_secs as u128 * want as u128) / have as u128).max(1) as u64;
        }
    }
    let base = (want as f64) / (1080.0 * 1920.0 * 97.0);
    ((FALLBACK_HERO_SECS as f64) * base).max(30.0) as u64
}

// ---------------------------------------------------------------------------
// farm.conf — the farm-wide limits, edited in one place
// ---------------------------------------------------------------------------
//
// farm.conf lives ON THE SHARE and every worker re-sources it each poll, so
// editing it here changes every Mac within one poll. It's bash, so the values
// are validated hard rather than trusted: a stray character in this file breaks
// every worker at once.

pub struct ConfKey {
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: &'static str, // int | choice | text
    pub choices: &'static [&'static str],
    pub min: u64,
    pub max: u64,
}

pub const CONF_KEYS: [ConfKey; 9] = [
    ConfKey { key: "PERF", label: "Default profile", help: "auto sizes itself to each Mac's RAM", kind: "choice", choices: &["auto", "full", "light"], min: 0, max: 0 },
    ConfKey { key: "MEM_BUDGET_PCT", label: "Memory budget %", help: "share of RAM a render may use", kind: "int", choices: &[], min: 40, max: 95 },
    ConfKey { key: "ADMISSION", label: "Admission control", help: "block = leave a job it can't afford for a bigger Mac", kind: "choice", choices: &["block", "warn"], min: 0, max: 0 },
    ConfKey { key: "MIN_FREE_GB", label: "Min free disk (GB)", help: "workers pause below this", kind: "int", choices: &[], min: 5, max: 500 },
    ConfKey { key: "POLL_SECS", label: "Poll interval (s)", help: "how often a worker looks for work", kind: "int", choices: &[], min: 5, max: 300 },
    ConfKey { key: "OOM_MAX_RETRY", label: "OOM retries", help: "requeues after a memory kill before giving up", kind: "int", choices: &[], min: 0, max: 5 },
    ConfKey { key: "OOM_BACKOFF", label: "OOM backoff (s)", help: "let memory drain before retrying", kind: "int", choices: &[], min: 10, max: 600 },
    ConfKey { key: "MAX_SWAP_USED_MB", label: "Max swap (MB)", help: "pause a Mac swapping more than this", kind: "int", choices: &[], min: 512, max: 65536 },
    ConfKey { key: "MODEL", label: "Model", help: "HuggingFace repo the workers load", kind: "text", choices: &[], min: 0, max: 0 },
];

fn conf_path(root: &str) -> PathBuf {
    Path::new(root).join("farm.conf")
}

/// Read the editable keys out of farm.conf, defaults included. The file is a
/// list of `: "${KEY:=VALUE}"` lines, which is also how a missing key gets its
/// default — so "not present" and "present with the default" mean the same.
pub fn read_farm_conf(root: &str) -> serde_json::Value {
    let body = std::fs::read_to_string(conf_path(root)).unwrap_or_default();
    let mut found: HashMap<String, String> = HashMap::new();
    for line in body.lines() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix(": \"${") else { continue };
        let Some((key, tail)) = rest.split_once(":=") else { continue };
        let val = tail.split('}').next().unwrap_or("").trim().to_string();
        found.insert(key.trim().to_string(), val);
    }
    let keys: Vec<serde_json::Value> = CONF_KEYS
        .iter()
        .map(|k| {
            serde_json::json!({
                "key": k.key, "label": k.label, "help": k.help, "kind": k.kind,
                "choices": k.choices, "min": k.min, "max": k.max,
                "value": found.get(k.key).cloned().unwrap_or_default(),
            })
        })
        .collect();
    serde_json::json!({
        "path": conf_path(root).to_string_lossy(),
        "exists": conf_path(root).is_file(),
        "keys": keys,
    })
}

fn valid_conf_value(k: &ConfKey, v: &str) -> Result<String, String> {
    let v = v.trim();
    match k.kind {
        "int" => {
            let n: u64 = v.parse().map_err(|_| format!("{} must be a whole number", k.label))?;
            if n < k.min || n > k.max {
                return Err(format!("{} must be between {} and {}", k.label, k.min, k.max));
            }
            Ok(n.to_string())
        }
        "choice" => {
            if !k.choices.contains(&v) {
                return Err(format!("{} must be one of: {}", k.label, k.choices.join(", ")));
            }
            Ok(v.to_string())
        }
        _ => {
            // MODEL ends up in a shell command on every worker. Nothing but a
            // HuggingFace-shaped repo id gets in.
            if v.is_empty() || v.len() > 120 {
                return Err(format!("{} looks wrong", k.label));
            }
            if !v.chars().all(|c| c.is_ascii_alphanumeric() || "._-/".contains(c)) {
                return Err(format!("{} may only contain letters, numbers, . _ - /", k.label));
            }
            Ok(v.to_string())
        }
    }
}

/// Write the given keys back into farm.conf, in place, keeping every comment.
/// Only the whitelisted keys can be touched and every value is validated first —
/// this file is sourced by bash on every Mac in the farm.
pub fn save_farm_conf(root: &str, patch: &serde_json::Value) -> Result<String, String> {
    let obj = patch.as_object().ok_or("expected an object of key -> value")?;
    let mut clean: Vec<(&ConfKey, String)> = Vec::new();
    for (k, v) in obj {
        let Some(def) = CONF_KEYS.iter().find(|c| c.key == k.as_str()) else {
            return Err(format!("{} isn't an editable farm setting", k));
        };
        let raw = match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            _ => return Err(format!("{} must be text or a number", k)),
        };
        clean.push((def, valid_conf_value(def, &raw)?));
    }
    if clean.is_empty() {
        return Err("nothing to change".into());
    }

    let path = conf_path(root);
    let body = std::fs::read_to_string(&path)
        .map_err(|e| format!("can't read {} — {}. Is the share mounted?", path.display(), e))?;
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for line in body.lines() {
        let mut replaced = None;
        if let Some(rest) = line.trim().strip_prefix(": \"${") {
            if let Some((key, _)) = rest.split_once(":=") {
                if let Some((def, val)) = clean.iter().find(|(d, _)| d.key == key.trim()) {
                    seen.insert(def.key);
                    // keep any trailing comment on the line
                    let comment = line.split_once('#').map(|(_, c)| format!("  # {}", c.trim())).unwrap_or_default();
                    replaced = Some(format!(": \"${{{}:={}}}\"{}", def.key, val, comment));
                }
            }
        }
        out.push(replaced.unwrap_or_else(|| line.to_string()));
    }
    for (def, val) in &clean {
        if !seen.contains(def.key) {
            out.push(format!(": \"${{{}:={}}}\"", def.key, val));
        }
    }
    let mut text = out.join("\n");
    text.push('\n');
    std::fs::write(&path, text).map_err(|e| format!("can't write {} — {}", path.display(), e))?;
    Ok(format!(
        "Updated {} setting(s) in farm.conf — every Mac picks it up within one poll.",
        clean.len()
    ))
}

// ---------------------------------------------------------------------------
// Farm-wide operations: reap, pause, resume
// ---------------------------------------------------------------------------

fn hold_dir(root: &str, hi: bool) -> PathBuf {
    if hi { Path::new(root).join("queue/hold/hi") } else { Path::new(root).join("queue/hold") }
}

/// Requeue jobs whose worker died. Same rule as `farm_status.sh --reap`: the
/// heartbeat (or the job file, for a worker too old to write one) hasn't been
/// touched in `stale_min` minutes. Kept in Rust so the board can offer it and
/// so autopilot can do it at 3am without a Terminal.
pub fn reap(root: &str, stale_min: u64) -> Result<Vec<String>, String> {
    let running = Path::new(root).join("running");
    if !running.is_dir() {
        return Err("the farm folder isn't reachable".into());
    }
    let queue = Path::new(root).join("queue");
    std::fs::create_dir_all(&queue).map_err(|e| e.to_string())?;
    let cutoff = stale_min.max(1) * 60;
    let mut reaped = Vec::new();

    for n in list_dir_all(&running) {
        if !n.contains(".job.") || n.ends_with(".heartbeat") || n.starts_with('.') {
            continue;
        }
        let job = running.join(&n);
        let hb = running.join(format!("{}.heartbeat", n));
        let age = if hb.exists() { mtime_age(&hb) } else { mtime_age(&job) };
        if age < cutoff {
            continue;
        }
        // Same marker farm_status.sh uses, so a reaped job is recognisable
        // whichever tool did it.
        let orig = format!("REQUEUED_{}", n.split(".job.").next().unwrap_or(&n));
        let to = queue.join(format!("{}.job", orig));
        if std::fs::rename(&job, &to).is_ok() {
            let _ = std::fs::remove_file(&hb);
            reaped.push(parse_id(&n));
        }
    }
    Ok(reaped)
}

/// Stop the farm taking new work without killing what's in flight: every
/// waiting job moves into queue/hold/, which the workers' glob can't see.
pub fn pause_queue(root: &str) -> Result<usize, String> {
    let mut moved = 0;
    for (from, hi) in [(Path::new(root).join("queue"), false), (Path::new(root).join("queue/hi"), true)] {
        let dir = hold_dir(root, hi);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        for n in list_dir_all(&from).into_iter().filter(|n| n.ends_with(".job")) {
            if std::fs::rename(from.join(&n), dir.join(&n)).is_ok() {
                moved += 1;
            }
        }
    }
    Ok(moved)
}

pub fn resume_queue(root: &str) -> Result<usize, String> {
    let mut moved = 0;
    for hi in [false, true] {
        let from = hold_dir(root, hi);
        let to = if hi { Path::new(root).join("queue/hi") } else { Path::new(root).join("queue") };
        std::fs::create_dir_all(&to).map_err(|e| e.to_string())?;
        for n in list_dir_all(&from).into_iter().filter(|n| n.ends_with(".job")) {
            if std::fs::rename(from.join(&n), to.join(&n)).is_ok() {
                moved += 1;
            }
        }
    }
    Ok(moved)
}

pub fn held_count(root: &str) -> usize {
    list_dir_all(&hold_dir(root, false)).iter().filter(|n| n.ends_with(".job")).count()
        + list_dir_all(&hold_dir(root, true)).iter().filter(|n| n.ends_with(".job")).count()
}

// ---------------------------------------------------------------------------
// Logs
// ---------------------------------------------------------------------------

/// The tail of a render log, plus a progress reading if the renderer prints one.
/// Works on both surfaces — the browser could fetch the file directly, but the
/// popover can't, and one path means one behaviour.
pub fn log_tail(root: &str, id: &str, host: &str, lines: usize) -> Result<serde_json::Value, String> {
    let name = format!("{}.{}.log", safe_id(id), crate::safe_host(host));
    let p = Path::new(root).join("logs").join(&name);
    let body = std::fs::read_to_string(&p)
        .map_err(|e| format!("no log at {} — {}", p.display(), e))?;
    let all: Vec<&str> = body.lines().collect();
    let tail: Vec<&str> = all.iter().rev().take(lines.clamp(20, 2000)).rev().cloned().collect();
    let (step, total) = parse_progress(&body);
    Ok(serde_json::json!({
        "path": p.to_string_lossy(),
        "lines": tail,
        "step": step,
        "total": total,
        "percent": if total > 0 { (step * 100 / total.max(1)).min(100) } else { 0 },
    }))
}

/// Pull a step counter out of a render log. Renderers print progress in several
/// shapes and none of them are guaranteed, so this reads the LAST thing that
/// looks like progress and reports nothing if there isn't one — an honest blank
/// beats a made-up percentage.
pub fn parse_progress(body: &str) -> (u64, u64) {
    let mut best = (0u64, 0u64);
    for line in body.lines().rev().take(400) {
        // "12/40", "step 12 of 40", "12%|" (tqdm)
        if let Some(p) = line.find('%') {
            let digits: String = line[..p].chars().rev().take_while(|c| c.is_ascii_digit()).collect();
            let pct: String = digits.chars().rev().collect();
            if let Ok(n) = pct.parse::<u64>() {
                if n <= 100 && n > 0 {
                    return (n, 100);
                }
            }
        }
        let cleaned = line.replace(" of ", "/");
        for (i, _) in cleaned.match_indices('/') {
            let left: String = cleaned[..i].chars().rev().take_while(|c| c.is_ascii_digit()).collect();
            let left: String = left.chars().rev().collect();
            let right: String = cleaned[i + 1..].chars().take_while(|c| c.is_ascii_digit()).collect();
            if let (Ok(a), Ok(b)) = (left.parse::<u64>(), right.parse::<u64>()) {
                if b > 1 && a <= b && b <= 100000 {
                    best = (a, b);
                }
            }
        }
        if best.1 > 0 {
            return best;
        }
    }
    best
}

// ---------------------------------------------------------------------------
// Runs — a named batch of work, e.g. everything queued for one night
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct RunManifest {
    pub run: String,
    pub note: String,
    pub by: String,
    pub created_ts: u64,
    pub planned: usize,
    pub proof_first: bool,
    pub sizes: Vec<String>,
    pub seeds: u32,
}

pub fn runs_dir(root: &str) -> PathBuf {
    Path::new(root).join("runs")
}

pub fn write_run(root: &str, m: &RunManifest) -> Result<(), String> {
    let dir = runs_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let body = serde_json::to_string_pretty(m).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{}.json", safe_id(&m.run))), body).map_err(|e| e.to_string())
}

/// Every run, with live progress counted off the lanes. Derived, never stored —
/// the manifest records intent, the folders record truth.
pub fn runs(root: &str) -> Vec<serde_json::Value> {
    let b = board(root, 2000);
    let mut out = Vec::new();
    let dir = runs_dir(root);
    let mut names: Vec<String> =
        list_dir_all(&dir).into_iter().filter(|n| n.ends_with(".json")).collect();
    names.sort_by_key(|n| mtime_age(&dir.join(n)));

    for n in names {
        let Ok(body) = std::fs::read_to_string(dir.join(&n)) else { continue };
        let Ok(m) = serde_json::from_str::<RunManifest>(&body) else { continue };
        let count = |v: &Vec<JobCard>| v.iter().filter(|c| c.run == m.run).count();
        let done = count(&b.done);
        let failed = count(&b.failed);
        let queued = count(&b.queued);
        let running = count(&b.running);
        let secs: u64 = b.done.iter().filter(|c| c.run == m.run).map(|c| c.duration_secs).sum();
        let left = queued + running;
        out.push(serde_json::json!({
            "run": m.run, "note": m.note, "by": m.by, "created_ts": m.created_ts,
            "planned": m.planned, "proof_first": m.proof_first,
            "queued": queued, "running": running, "done": done, "failed": failed,
            "render_secs": secs,
            "finished": left == 0 && (done + failed) > 0,
        }));
    }
    out
}

/// The morning report for one run: what landed, what didn't, who rendered what.
pub fn run_report(root: &str, run: &str) -> serde_json::Value {
    let b = board(root, 2000);
    let mine = |v: &Vec<JobCard>| -> Vec<JobCard> {
        v.iter().filter(|c| c.run == run).cloned().collect()
    };
    let done = mine(&b.done);
    let failed = mine(&b.failed);
    let queued = mine(&b.queued);
    let running = mine(&b.running);

    let mut per_host: HashMap<String, usize> = HashMap::new();
    for c in &done {
        *per_host.entry(c.host.clone()).or_insert(0) += 1;
    }
    let mut hosts: Vec<serde_json::Value> = per_host
        .into_iter()
        .map(|(h, n)| serde_json::json!({ "host": h, "clips": n }))
        .collect();
    hosts.sort_by(|a, b| b["clips"].as_u64().cmp(&a["clips"].as_u64()));

    let render_secs: u64 = done.iter().map(|c| c.duration_secs).sum();
    let approved = done.iter().filter(|c| c.review == "approved").count();
    let retake = done.iter().filter(|c| c.review == "retake").count();

    serde_json::json!({
        "run": run,
        "done": done, "failed": failed, "queued": queued, "running": running,
        "counts": {
            "done": done.len(), "failed": failed.len(),
            "queued": queued.len(), "running": running.len(),
            "approved": approved, "retake": retake,
        },
        "render_secs": render_secs,
        "per_host": hosts,
        "finished": queued.is_empty() && running.is_empty(),
    })
}

/// What the overnight planner takes: a list of prompts and how to spend the night.
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct RunPlan {
    pub run: String,
    pub note: String,
    pub prompts: Vec<String>,
    pub sizes: Vec<String>, // "1080x1920"
    pub seeds: u32,         // takes per prompt
    pub frames: u32,
    pub fps: u32,
    pub mode: String,       // hero | test  (test = proofs to cherry-pick in the morning)
    pub perf: String,
    pub priority: String,
    pub member: String,
    pub kind: String,
    pub image: String,
    pub lora: String,
    pub still_prompt: String,
}

/// Turn a list of prompts into a whole night of work, in claim order.
///
/// This is the "paste the shot list and go home" path: N prompts × M sizes ×
/// K seeds, all tagged with one run name so the morning report can add it up.
/// Ordered prompt-major so an interrupted night still leaves every prompt
/// represented rather than 40 versions of the first one.
pub fn plan_run(root: &str, plan: &RunPlan, stamp: &str) -> Result<serde_json::Value, String> {
    let prompts: Vec<String> = plan
        .prompts
        .iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    if prompts.is_empty() {
        return Err("Paste at least one prompt — one per line.".into());
    }
    if prompts.len() > 200 {
        return Err("That's more than 200 prompts; split it into a couple of runs.".into());
    }

    let sizes: Vec<(u32, u32)> = {
        let mut v: Vec<(u32, u32)> = plan
            .sizes
            .iter()
            .filter_map(|s| {
                let (w, h) = s.split_once('x')?;
                Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
            })
            .collect();
        if v.is_empty() {
            v.push((1080, 1920));
        }
        v
    };
    let seeds = plan.seeds.clamp(1, 12);
    let total = prompts.len() * sizes.len() * seeds as usize;
    if total > 600 {
        return Err(format!(
            "{} jobs is more than one night — trim the prompts, sizes or takes.",
            total
        ));
    }

    let run = if plan.run.trim().is_empty() {
        format!("run_{}", stamp)
    } else {
        safe_id(&plan.run)
    };
    let mode = if plan.mode == "test" { "test" } else { "hero" };

    let mut written: Vec<String> = Vec::new();
    let mut n = 0usize;
    for (pi, prompt) in prompts.iter().enumerate() {
        for (w, h) in &sizes {
            for k in 0..seeds {
                n += 1;
                let id = format!(
                    "{}_{:02}{}{}",
                    run,
                    pi + 1,
                    if sizes.len() > 1 { format!("_{}x{}", w, h) } else { String::new() },
                    if seeds > 1 { format!("_s{}", 1000 + k) } else { String::new() }
                );
                let job = NewJob {
                    id,
                    prompt: prompt.clone(),
                    kind: if plan.kind.is_empty() { "t2v".into() } else { plan.kind.clone() },
                    mode: mode.into(),
                    image: plan.image.clone(),
                    lora: plan.lora.clone(),
                    still_prompt: plan.still_prompt.clone(),
                    width: *w,
                    height: *h,
                    frames: if plan.frames == 0 { 97 } else { plan.frames },
                    seed: if seeds > 1 { 1000 + k as i64 } else { 42 },
                    fps: if plan.fps == 0 { 24 } else { plan.fps },
                    perf: plan.perf.clone(),
                    priority: plan.priority.clone(),
                    member: plan.member.clone(),
                    run: run.clone(),
                    ..Default::default()
                };
                // Stamped in creation order so the farm works through the list
                // in the order it was written, not alphabetically by prompt.
                let mut files = enqueue(root, &job, &format!("{}_{:04}", stamp, n))?;
                written.append(&mut files);
            }
        }
    }

    write_run(root, &RunManifest {
        run: run.clone(),
        note: plan.note.clone(),
        by: plan.member.clone(),
        created_ts: 0, // stamped by the caller, which knows the clock
        planned: written.len(),
        proof_first: mode == "test",
        sizes: plan.sizes.clone(),
        seeds,
    })?;

    Ok(serde_json::json!({
        "run": run,
        "queued": written.len(),
        "message": format!(
            "Queued {} job(s) as “{}”{}. {}",
            written.len(),
            run,
            if mode == "test" { " as proof stills" } else { "" },
            if mode == "test" {
                "Cherry-pick the winners in Review in the morning."
            } else {
                "The farm will work top-down through the night."
            }
        ),
    }))
}

// ---------------------------------------------------------------------------
// Autopilot — what keeps a night unattended
// ---------------------------------------------------------------------------

/// Everything autopilot may do in one pass, and why. Returned so it can be
/// logged to the share and shown in the morning: an unattended system that
/// can't explain what it did overnight is not trustworthy.
#[derive(Serialize, Default)]
pub struct AutoResult {
    pub reaped: Vec<String>,
    pub retried: Vec<String>,
    pub gave_up: Vec<String>,
    pub paused: bool,
    pub reason: String,
}

impl AutoResult {
    pub fn did_something(&self) -> bool {
        !self.reaped.is_empty() || !self.retried.is_empty() || !self.gave_up.is_empty() || self.paused
    }
    pub fn summary(&self) -> String {
        let mut bits: Vec<String> = Vec::new();
        if !self.reaped.is_empty() {
            bits.push(format!("requeued {} stalled", self.reaped.len()));
        }
        if !self.retried.is_empty() {
            bits.push(format!("retried {}", self.retried.len()));
        }
        if !self.gave_up.is_empty() {
            bits.push(format!("gave up on {}", self.gave_up.len()));
        }
        if self.paused {
            bits.push(format!("paused the queue ({})", self.reason));
        }
        bits.join(", ")
    }
}

/// How autopilot is allowed to behave. All off by default: nothing touches the
/// farm unattended until someone ticks the box on one Mac.
pub struct AutoPolicy {
    pub stale_min: u64,      // requeue an in-flight job whose worker went quiet
    pub max_retry: u32,      // re-run a failure this many times
    pub fail_streak: u32,    // pause the whole queue after this many in a row
    pub member: String,
}

// A failure that is clearly about memory gets treated differently from one that
// isn't: retrying an OOM at the same size on the same Macs just burns the night.
fn is_memory_rc(rc: &str) -> bool {
    matches!(rc, "137" | "134" | "9")
}

fn biggest_ram_gb(root: &str) -> u64 {
    members(root).iter().map(|m| m.ram_gb).max().unwrap_or(0)
}

/// One autopilot pass. Deliberately conservative: it only ever requeues work,
/// never deletes it, and it stops the farm rather than looping on a fault.
pub fn autopilot_tick(root: &str, pol: &AutoPolicy, stamp: &str) -> AutoResult {
    let mut out = AutoResult::default();
    if !Path::new(root).is_dir() {
        return out;
    }

    // 1. a Mac that died mid-render shouldn't cost us the job
    if let Ok(reaped) = reap(root, pol.stale_min) {
        out.reaped = reaped;
    }

    let b = board(root, 400);

    // 2. a run of consecutive failures means something is broken, not unlucky.
    //    Pausing beats spending the remaining six hours failing 200 times.
    let recent: Vec<&JobCard> = b.failed.iter().take(pol.fail_streak.max(1) as usize).collect();
    let all_fresh = recent.len() >= pol.fail_streak.max(1) as usize
        && recent.iter().all(|c| c.age_secs < 3 * 3600);
    if all_fresh && !b.queued.is_empty() {
        if let Ok(n) = pause_queue(root) {
            out.paused = true;
            out.reason = format!("{} failures in a row", recent.len());
            out.reason = format!("{}, held {} job(s)", out.reason, n);
            return out; // don't also retry into a wall
        }
    }

    // 3. retry the failures worth retrying
    for c in &b.failed {
        if c.retry >= pol.max_retry {
            continue;
        }
        if c.review == "retake" {
            continue; // a human already has plans for this one
        }
        let mut job = base_from(c);
        job.member = if c.member.is_empty() { pol.member.clone() } else { c.member.clone() };
        job.run = c.run.clone();
        job.retry = c.retry + 1;
        job.id = c.id.clone();

        if is_memory_rc(&c.rc) {
            // Memory kill: ask for a bigger Mac if the farm has one, otherwise
            // drop the resolution rather than repeat the same death.
            let big = biggest_ram_gb(root);
            if big > c.min_ram_gb && big > 0 {
                job.min_ram_gb = big;
            } else {
                job.width = (c.width * 2 / 3).max(544) / 8 * 8;
                job.height = (c.height * 2 / 3).max(544) / 8 * 8;
                job.perf = "light".into();
            }
        }

        let name = format!("{}_r{}", stamp, job.retry);
        match enqueue(root, &job, &name) {
            Ok(_) => {
                // move the failed record aside so it isn't retried forever
                let from = Path::new(root).join("failed").join(&c.file);
                let to = Path::new(root).join("failed").join(format!("retried_{}", c.file));
                let _ = std::fs::rename(from, to);
                out.retried.push(c.id.clone());
            }
            Err(_) => out.gave_up.push(c.id.clone()),
        }
    }
    out
}

/// Only one Mac should act as the babysitter, or two of them will requeue the
/// same job twice. A lock file on the share with a heartbeat decides it.
pub fn claim_supervisor(root: &str, host: &str, now: u64) -> bool {
    let dir = runs_dir(root);
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    let p = dir.join(".autopilot.lock");
    let held_by_other = std::fs::read_to_string(&p)
        .ok()
        .and_then(|b| serde_json::from_str::<serde_json::Value>(&b).ok())
        .map(|v| {
            let who = v["host"].as_str().unwrap_or("").to_string();
            let age = mtime_age(&p);
            !who.eq_ignore_ascii_case(host) && age < 120
        })
        .unwrap_or(false);
    if held_by_other {
        return false;
    }
    let body = serde_json::json!({ "host": host, "ts": now }).to_string();
    std::fs::write(&p, body).is_ok()
}

/// Autopilot's diary, on the share, so the morning can see what happened at 3am.
pub fn log_autopilot(root: &str, host: &str, line: &str) {
    let dir = Path::new(root).join("logs");
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("autopilot.log");
    let stamp = sh("date '+%Y-%m-%d %H:%M:%S'").trim().to_string();
    let entry = format!("{}  [{}] {}\n", stamp, host, line);
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = f.write_all(entry.as_bytes());
    }
}

pub fn autopilot_log_tail(root: &str, lines: usize) -> Vec<String> {
    let p = Path::new(root).join("logs/autopilot.log");
    let body = std::fs::read_to_string(p).unwrap_or_default();
    let all: Vec<&str> = body.lines().collect();
    all.iter().rev().take(lines.clamp(5, 500)).rev().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_job_file_including_escaped_prompts() {
        let body = "# comment\nID=\"hail_hero\"\nTYPE=\"t2v\"\nPROMPT=\"a \\\"big\\\" storm\"\nWIDTH=1080\nHEIGHT=1920\nFRAMES=97\nSEED=8804\n";
        let kv = parse_job_kv(body);
        assert_eq!(kv["ID"], "hail_hero");
        assert_eq!(kv["PROMPT"], "a \"big\" storm");
        assert_eq!(kv["SEED"], "8804");
    }

    #[test]
    fn frames_snap_to_the_8k_plus_1_rule() {
        assert_eq!(snap_frames(97), 97);
        assert_eq!(snap_frames(100), 97);
        assert_eq!(snap_frames(96), 89);
        assert_eq!(snap_frames(1), 9);
        assert_eq!(snap_frames(9999), 481);
    }

    #[test]
    fn ids_stay_filename_safe() {
        assert_eq!(safe_id("hail hero/../x"), "hail_hero____x");
        assert_eq!(safe_id("   "), "job");
        assert!(!safe_id("a".repeat(200).as_str()).contains('/'));
        assert_eq!(safe_id("a".repeat(200).as_str()).len(), 60);
    }

    // A prompt is sourced by bash on the worker. Nothing in it may execute.
    #[test]
    fn prompts_cannot_escape_into_a_command() {
        let j = NewJob { prompt: "storm\"; rm -rf ~; echo \"$(whoami) `id`".into(), ..Default::default() };
        let body = job_body(&j, 42);
        let line = body.lines().find(|l| l.starts_with("PROMPT=")).unwrap();
        // Every shell metacharacter must arrive backslash-escaped, so bash sees
        // literal text where a command substitution used to be.
        assert!(!line.contains("$(") || line.contains("\\$("), "command substitution survived: {}", line);
        assert!(line.contains("\\$("), "the $ must be escaped: {}", line);
        assert!(line.contains("\\`id\\`"), "backticks must be escaped: {}", line);
        assert!(!line.replace("\\$", "").contains('$'), "an unescaped $ survived: {}", line);
        // exactly one opening and one closing quote around the value
        assert!(line.starts_with("PROMPT=\"") && line.ends_with('"'));
        assert_eq!(line.matches("\\\"").count(), 2, "inner quotes must be escaped: {}", line);
    }

    #[test]
    fn a_job_written_here_parses_back_the_same() {
        let j = NewJob { id: "demo".into(), prompt: "a dragon".into(), width: 1080, height: 1080, frames: 100, ..Default::default() };
        let kv = parse_job_kv(&job_body(&j, 7));
        assert_eq!(kv["ID"], "demo");
        assert_eq!(kv["HEIGHT"], "1080");
        assert_eq!(kv["FRAMES"], "97", "100 is not 8k+1 and must be snapped");
        assert_eq!(kv["SEED"], "7");
        assert_eq!(kv["MODE"], "hero");
    }

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("ltxjobs_{}", name));
        let _ = std::fs::remove_dir_all(&p);
        for d in ["queue", "queue/hi", "running", "done", "failed", "presence", "logs"] {
            std::fs::create_dir_all(p.join(d)).unwrap();
        }
        p
    }

    #[test]
    fn board_reads_every_lane_in_claim_order() {
        let root = tmp("board");
        let r = root.to_string_lossy().to_string();
        std::fs::write(root.join("queue/20260101_010101_1__later.job"), "ID=\"later\"\nPROMPT=\"b\"\n").unwrap();
        std::fs::write(root.join("queue/20250101_010101_1__early.job"), "ID=\"early\"\nPROMPT=\"a\"\n").unwrap();
        std::fs::write(root.join("queue/hi/20270101_010101_1__urgent.job"), "ID=\"urgent\"\nPROMPT=\"c\"\n").unwrap();
        std::fs::write(root.join("running/20260101_010101_1__live.job.MAC1.99"), "ID=\"live\"\nPROMPT=\"d\"\n").unwrap();
        std::fs::write(root.join("done/20260101_010101_1__fin.job.MAC2.99.ok"), "ID=\"fin\"\nPROMPT=\"e\"\n").unwrap();
        std::fs::write(root.join("done/fin.mp4"), vec![0u8; 2_100_000]).unwrap();
        std::fs::write(root.join("done/fin.json"), "{\"duration_secs\":123,\"peak_mem_gb\":41.5}").unwrap();
        std::fs::write(root.join("failed/20260101_010101_1__bad.job.MAC3.99.rc137"), "ID=\"bad\"\nPROMPT=\"f\"\n").unwrap();

        let b = board(&r, 50);
        assert!(b.reachable);
        // hi/ lane is claimed first even though its stamp is newest
        assert_eq!(b.queued.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), vec!["urgent", "early", "later"]);
        assert_eq!(b.queued[0].priority, "high");
        assert_eq!(b.queued[0].position, 1);
        assert_eq!(b.running[0].host, "MAC1");
        assert_eq!(b.done[0].host, "MAC2");
        assert_eq!(b.done[0].mp4_mb, 2);
        assert_eq!(b.done[0].duration_secs, 123);
        assert_eq!(b.failed[0].rc, "137");
        assert_eq!(b.queued[0].aspect, "9:16");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reorder_puts_the_dragged_order_ahead_of_new_jobs() {
        let root = tmp("reorder");
        let r = root.to_string_lossy().to_string();
        for id in ["a", "b", "c"] {
            std::fs::write(root.join(format!("queue/20260101_01010{}_1__{}.job", id.len(), id)), format!("ID=\"{}\"\nPROMPT=\"p\"\n", id)).unwrap();
        }
        let before: Vec<String> = board(&r, 10).queued.iter().map(|c| c.file.clone()).collect();
        // drag the last one to the front
        let want = vec![before[2].clone(), before[0].clone(), before[1].clone()];
        reorder_queue(&r, &want).unwrap();
        let after: Vec<String> = board(&r, 10).queued.iter().map(|c| c.id.clone()).collect();
        assert_eq!(after, vec!["c", "a", "b"]);

        // and a job enqueued afterwards lands behind the hand-picked order
        enqueue(&r, &NewJob { id: "new".into(), prompt: "x".into(), ..Default::default() }, "20270101_000000").unwrap();
        let after2: Vec<String> = board(&r, 10).queued.iter().map(|c| c.id.clone()).collect();
        assert_eq!(after2, vec!["c", "a", "b", "new"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn priority_moves_between_lanes_and_back() {
        let root = tmp("prio");
        let r = root.to_string_lossy().to_string();
        std::fs::write(root.join("queue/20260101_010101_1__x.job"), "ID=\"x\"\nPROMPT=\"p\"\n").unwrap();
        let f = board(&r, 10).queued[0].file.clone();
        set_priority(&r, &f, true).unwrap();
        assert!(root.join("queue/hi").join(&f).is_file());
        assert_eq!(board(&r, 10).queued[0].priority, "high");
        set_priority(&r, &f, false).unwrap();
        assert!(root.join("queue").join(&f).is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn requeue_strips_the_worker_suffix_and_keeps_the_original() {
        let root = tmp("requeue");
        let r = root.to_string_lossy().to_string();
        std::fs::write(root.join("failed/20260101_010101_1__bad.job.MAC.9.rc137"), "ID=\"bad\"\nPROMPT=\"p\"\n").unwrap();
        requeue_job(&r, "failed", "20260101_010101_1__bad.job.MAC.9.rc137", "20260202_020202").unwrap();
        assert!(root.join("queue/20260202_020202__bad.job").is_file());
        assert!(!root.join("failed/20260101_010101_1__bad.job.MAC.9.rc137").exists(), "failed job moves, it isn't copied");

        std::fs::write(root.join("done/20260101_010101_1__fin.job.MAC.9.ok"), "ID=\"fin\"\nPROMPT=\"p\"\n").unwrap();
        requeue_job(&r, "done", "20260101_010101_1__fin.job.MAC.9.ok", "20260202_020203").unwrap();
        assert!(root.join("queue/20260202_020203__fin.job").is_file());
        assert!(root.join("done/20260101_010101_1__fin.job.MAC.9.ok").is_file(), "a finished job stays in done/");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cancel_refuses_a_claimed_job() {
        let root = tmp("cancel");
        let r = root.to_string_lossy().to_string();
        std::fs::write(root.join("running/20260101_010101_1__live.job.MAC.9"), "ID=\"live\"\n").unwrap();
        let err = cancel_job(&r, "20260101_010101_1__live.job.MAC.9").unwrap_err();
        assert!(err.contains("claimed"), "{}", err);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn path_traversal_is_refused_everywhere() {
        let root = tmp("traversal");
        let r = root.to_string_lossy().to_string();
        assert!(cancel_job(&r, "../../etc/passwd").is_err());
        assert!(requeue_job(&r, "failed", "../x", "1").is_err());
        assert!(set_priority(&r, "../x", true).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_sweep_writes_one_file_per_seed() {
        let root = tmp("sweep");
        let r = root.to_string_lossy().to_string();
        let files = enqueue(&r, &NewJob { id: "dragon".into(), prompt: "a dragon".into(), sweep: 4, ..Default::default() }, "20260101_000000").unwrap();
        assert_eq!(files.len(), 4);
        let b = board(&r, 10);
        assert_eq!(b.queued.len(), 4);
        let seeds: Vec<i64> = b.queued.iter().map(|c| c.seed).collect();
        assert_eq!(seeds, vec![1000, 1001, 1002, 1003]);
        assert!(b.queued[0].id.starts_with("dragon_s"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn high_priority_enqueue_lands_in_the_hi_lane() {
        let root = tmp("hienq");
        let r = root.to_string_lossy().to_string();
        enqueue(&r, &NewJob { id: "urgent".into(), prompt: "p".into(), priority: "high".into(), ..Default::default() }, "20260101_000000").unwrap();
        assert_eq!(board(&r, 10).queued[0].priority, "high");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn enqueue_without_a_prompt_is_refused() {
        let root = tmp("noprompt");
        let r = root.to_string_lossy().to_string();
        assert!(enqueue(&r, &NewJob { id: "x".into(), ..Default::default() }, "20260101_000000").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn variants_cover_the_other_sizes_and_never_repeat_the_original() {
        let card = JobCard {
            id: "hail_hero".into(),
            prompt: "storm clouds over a QLD roof".into(),
            width: 1080,
            height: 1920,
            frames: 97,
            seed: 8804,
            fps: 24,
            kind: "t2v".into(),
            mode: "hero".into(),
            ..Default::default()
        };
        let v = variants_for(&card);
        let sizes: Vec<&Variant> = v.iter().filter(|x| x.group == "size").collect();
        assert_eq!(sizes.len(), 3, "4 delivery sizes minus the one it already is");
        assert!(!sizes.iter().any(|s| s.job["width"] == 1080 && s.job["height"] == 1920));
        assert!(sizes.iter().all(|s| s.job["prompt"] == "storm clouds over a QLD roof"));

        let prompts: Vec<&Variant> = v.iter().filter(|x| x.group == "prompt").collect();
        assert_eq!(prompts.len(), 5);
        assert!(prompts.iter().all(|p| p.job["prompt"].as_str().unwrap().starts_with("storm clouds over a QLD roof,")));
        assert!(prompts.iter().all(|p| p.job["seed"] == 8804), "a prompt tweak must keep the seed to stay comparable");

        assert!(v.iter().any(|x| x.group == "seed" && x.job["sweep"] == 4));
        assert!(v.iter().any(|x| x.group == "quality" && x.job["mode"] == "test"));
        // every variant must be enqueueable
        for x in &v {
            let j: NewJob = serde_json::from_value(x.job.clone()).expect("variant is a valid NewJob");
            assert!(!j.prompt.trim().is_empty());
            assert_eq!(snap_frames(j.frames), j.frames);
        }
    }

    #[test]
    fn members_merge_app_presence_worker_info_and_heartbeats() {
        let root = tmp("members");
        let r = root.to_string_lossy().to_string();
        write_presence(&r, &Presence {
            host: "MAC1".into(), member: "Aiden".into(), model: "Mac16,10".into(),
            ram_gb: 64, role: "coordinator".into(), perf: "full".into(),
            gateway: "http://mac1.local:8787/".into(), ..Default::default()
        }).unwrap();
        std::fs::write(root.join("running/.worker.MAC1.info"),
            "HOST=\"MAC1\"\nRAM_GB=64\nPERF=\"full\"\nBUDGET_GB=57.6\nFREE_PCT=41\nPRESSURE=1\nSWAP_MB=0\nSTATE=\"rendering\"\n").unwrap();
        std::fs::write(root.join("running/20260101_010101_1__live.job.MAC1.99"), "ID=\"live\"\nPROMPT=\"a storm\"\n").unwrap();
        std::fs::write(root.join("running/20260101_010101_1__live.job.MAC1.99.heartbeat"), "").unwrap();
        std::fs::write(root.join("done/20260101_010101_1__x.job.MAC1.9.ok"), "ID=\"x\"\n").unwrap();
        // a headless worker with no app running
        std::fs::write(root.join("running/.worker.MAC2.info"),
            "HOST=\"MAC2\"\nRAM_GB=32\nPERF=\"light\"\nSTATE=\"paused:disk\"\n").unwrap();

        let m = members(&r);
        assert_eq!(m.len(), 2, "one row per Mac, not one per file");
        let mac1 = m.iter().find(|x| x.host == "MAC1").unwrap();
        assert_eq!(mac1.member, "Aiden");
        assert_eq!(mac1.state, "rendering");
        assert_eq!(mac1.job, "live");
        assert_eq!(mac1.job_prompt, "a storm");
        assert_eq!(mac1.ram_gb, 64);
        assert_eq!(mac1.done_count, 1);
        assert!(mac1.app && mac1.worker);
        assert_eq!(mac1.gateway, "http://mac1.local:8787/");

        let mac2 = m.iter().find(|x| x.host == "MAC2").unwrap();
        assert_eq!(mac2.state, "paused");
        assert_eq!(mac2.detail, "paused — low disk");
        assert!(!mac2.app, "no presence file means the app isn't running there");
        assert!(mac2.worker);
        let _ = std::fs::remove_dir_all(&root);
    }

    // --- review + proofs -------------------------------------------------

    #[test]
    fn a_review_survives_a_round_trip_and_can_be_cleared() {
        let root = tmp("review");
        let r = root.to_string_lossy().to_string();
        write_review(&r, &Review { id: "clip_a".into(), state: "approved".into(), by: "Aiden".into(), note: "use this one".into(), ts: 5 }).unwrap();
        let back = read_review(&r, "clip_a").unwrap();
        assert_eq!(back.state, "approved");
        assert_eq!(back.note, "use this one");
        // it lands on the done card too
        std::fs::write(root.join("done/20260101_010101_1__clip_a.job.MAC.9.ok"), "ID=\"clip_a\"\nPROMPT=\"p\"\n").unwrap();
        assert_eq!(board(&r, 10).done[0].review, "approved");

        write_review(&r, &Review { id: "clip_a".into(), state: String::new(), ..Default::default() }).unwrap();
        assert!(read_review(&r, "clip_a").is_none(), "an empty state clears the review");
        assert!(write_review(&r, &Review { id: "clip_a".into(), state: "lol".into(), ..Default::default() }).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn proofs_carry_their_prompt_and_know_if_a_hero_exists() {
        let root = tmp("proofs");
        let r = root.to_string_lossy().to_string();
        std::fs::create_dir_all(root.join("done/proofs")).unwrap();
        std::fs::write(root.join("done/proofs/shot_a_seed1000.png"), b"x").unwrap();
        std::fs::write(root.join("done/proofs/shot_b_seed42.png"), b"x").unwrap();
        // shot_a was a test job; shot_b already has a hero render
        std::fs::write(root.join("done/20260101_010101_1__shot_a.job.MAC.9.ok"),
            "ID=\"shot_a\"\nPROMPT=\"a roof\"\nMODE=\"test\"\nSEED=1000\n").unwrap();
        std::fs::write(root.join("done/20260101_010102_1__shot_b.job.MAC.9.ok"),
            "ID=\"shot_b\"\nPROMPT=\"a gutter\"\nMODE=\"hero\"\nSEED=42\n").unwrap();

        let ps = proofs(&r, 10);
        assert_eq!(ps.len(), 2);
        let a = ps.iter().find(|p| p.id == "shot_a").unwrap();
        assert_eq!(a.prompt, "a roof");
        assert_eq!(a.seed, 1000);
        assert!(!a.rendered, "no hero render of shot_a exists yet");
        let b = ps.iter().find(|p| p.id == "shot_b").unwrap();
        assert!(b.rendered, "shot_b has a hero render, so it shouldn't beg to be rendered again");
        let _ = std::fs::remove_dir_all(&root);
    }

    // --- assets ----------------------------------------------------------

    #[test]
    fn uploads_are_confined_to_assets_and_to_images() {
        let root = tmp("assets");
        let r = root.to_string_lossy().to_string();
        let p = asset_target(&r, "Hail Shot 01.PNG").unwrap();
        assert!(p.ends_with("Hail_Shot_01.PNG") || p.ends_with("Hail_Shot_01.png"), "{:?}", p);
        assert!(p.starts_with(root.join("assets")));
        // a traversal attempt keeps only the basename
        let p = asset_target(&r, "../../evil.png").unwrap();
        assert_eq!(p.parent().unwrap(), root.join("assets"));
        assert!(asset_target(&r, "payload.sh").is_err());
        assert!(asset_target(&r, "noext").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn assets_and_loras_are_listed_for_the_composer() {
        let root = tmp("assetlist");
        let r = root.to_string_lossy().to_string();
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::create_dir_all(root.join("loras")).unwrap();
        std::fs::write(root.join("assets/a.png"), b"x").unwrap();
        std::fs::write(root.join("assets/notes.txt"), b"x").unwrap();
        std::fs::write(root.join("loras/Elijah_lora.safetensors"), b"x").unwrap();
        let v = list_assets(&r);
        assert_eq!(v["images"].as_array().unwrap().len(), 1);
        assert_eq!(v["loras"][0], "Elijah_lora.safetensors");
        let _ = std::fs::remove_dir_all(&root);
    }

    // --- stats + estimates ------------------------------------------------

    fn sidecar(root: &Path, id: &str, w: u32, h: u32, f: u32, mode: &str, host: &str, secs: u64, peak: f64, budget: f64) {
        let body = serde_json::json!({
            "id": id, "mode": mode, "type": "t2v", "width": w, "height": h, "frames": f,
            "worker": host, "duration_secs": secs, "peak_mem_gb": peak, "budget_gb": budget
        });
        std::fs::write(root.join(format!("done/{}.json", id)), body.to_string()).unwrap();
    }

    #[test]
    fn stats_add_up_per_mac_and_per_size() {
        let root = tmp("stats");
        let r = root.to_string_lossy().to_string();
        sidecar(&root, "a", 1080, 1920, 97, "hero", "MAC1", 1800, 41.0, 57.6);
        sidecar(&root, "b", 1080, 1920, 97, "hero", "MAC1", 1600, 44.0, 57.6);
        sidecar(&root, "c", 1080, 1080, 97, "hero", "MAC2", 900, 60.0, 57.6);
        let st = stats(&r);
        assert_eq!(st.clips, 3);
        assert_eq!(st.avg_secs, (1800 + 1600 + 900) / 3);
        let mac1 = st.per_host.iter().find(|h| h.host == "MAC1").unwrap();
        assert_eq!(mac1.clips, 2);
        assert_eq!(mac1.avg_secs, 1700);
        let vert = st.by_size.iter().find(|z| z.width == 1080 && z.height == 1920).unwrap();
        assert_eq!(vert.clips, 2);
        assert_eq!(vert.avg_secs, 1700);
        assert_eq!(st.over_budget, 1, "MAC2's render peaked above its budget");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn estimates_use_history_then_scale_then_fall_back() {
        let root = tmp("est");
        let r = root.to_string_lossy().to_string();
        // no history at all: a rough but sane number, scaled by size
        let empty = stats(&r);
        let big = estimate_secs(&empty, 1080, 1920, 97, "hero");
        let small = estimate_secs(&empty, 540, 960, 49, "hero");
        assert!(big > small, "a smaller job must estimate smaller ({} vs {})", big, small);
        assert_eq!(estimate_secs(&empty, 1080, 1920, 97, "test"), 90, "a proof still isn't a video render");

        sidecar(&root, "a", 1080, 1920, 97, "hero", "MAC1", 1200, 40.0, 57.6);
        let st = stats(&r);
        assert_eq!(estimate_secs(&st, 1080, 1920, 97, "hero"), 1200, "exact match wins");
        // half the pixels, half the frames -> roughly a quarter of the work
        let scaled = estimate_secs(&st, 540, 960, 49, "hero");
        assert!(scaled > 100 && scaled < 400, "scaled estimate was {}", scaled);
        let _ = std::fs::remove_dir_all(&root);
    }

    // --- farm.conf --------------------------------------------------------

    fn conf_fixture(root: &Path) {
        std::fs::write(root.join("farm.conf"),
            "# the farm's limits\n: \"${PERF:=auto}\"\n: \"${MEM_BUDGET_PCT:=90}\"\n: \"${ADMISSION:=block}\"   # block | warn\n: \"${MODEL:=dgrauet/ltx-2.3-mlx-q4}\"\n").unwrap();
    }

    #[test]
    fn farm_conf_reads_its_defaults_and_writes_in_place() {
        let root = tmp("conf");
        let r = root.to_string_lossy().to_string();
        conf_fixture(&root);
        let v = read_farm_conf(&r);
        assert_eq!(v["exists"], true);
        let keys = v["keys"].as_array().unwrap();
        let pct = keys.iter().find(|k| k["key"] == "MEM_BUDGET_PCT").unwrap();
        assert_eq!(pct["value"], "90");

        save_farm_conf(&r, &serde_json::json!({ "MEM_BUDGET_PCT": 80, "ADMISSION": "warn" })).unwrap();
        let body = std::fs::read_to_string(root.join("farm.conf")).unwrap();
        assert!(body.contains(": \"${MEM_BUDGET_PCT:=80}\""), "{}", body);
        assert!(body.contains(": \"${ADMISSION:=warn}\""), "{}", body);
        assert!(body.contains("# the farm's limits"), "comments must survive");
        assert!(body.contains("# block | warn"), "trailing comments must survive");
        // a key the file doesn't have yet gets appended
        save_farm_conf(&r, &serde_json::json!({ "POLL_SECS": 30 })).unwrap();
        assert!(std::fs::read_to_string(root.join("farm.conf")).unwrap().contains("POLL_SECS:=30"));
        let _ = std::fs::remove_dir_all(&root);
    }

    // This file is sourced by bash on EVERY Mac. A bad value here breaks the
    // whole farm at once, so nothing unvalidated may reach it.
    #[test]
    fn farm_conf_refuses_anything_that_could_break_a_worker() {
        let root = tmp("confsafe");
        let r = root.to_string_lossy().to_string();
        conf_fixture(&root);
        for bad in [
            serde_json::json!({ "MODEL": "repo; rm -rf ~" }),
            serde_json::json!({ "MODEL": "$(whoami)" }),
            serde_json::json!({ "MEM_BUDGET_PCT": "ninety" }),
            serde_json::json!({ "MEM_BUDGET_PCT": 400 }),
            serde_json::json!({ "ADMISSION": "maybe" }),
            serde_json::json!({ "PATH": "/tmp" }),
            serde_json::json!({ "POLL_SECS": 0 }),
        ] {
            assert!(save_farm_conf(&r, &bad).is_err(), "accepted {:?}", bad);
        }
        let body = std::fs::read_to_string(root.join("farm.conf")).unwrap();
        assert!(body.contains("ADMISSION:=block"), "the file must be untouched: {}", body);
        let _ = std::fs::remove_dir_all(&root);
    }

    // --- reap / pause / resume -------------------------------------------

    #[test]
    fn reap_only_takes_jobs_whose_worker_went_quiet() {
        let root = tmp("reap");
        let r = root.to_string_lossy().to_string();
        // live: heartbeat touched just now
        std::fs::write(root.join("running/20260101_010101_1__live.job.MAC.1"), "ID=\"live\"\n").unwrap();
        std::fs::write(root.join("running/20260101_010101_1__live.job.MAC.1.heartbeat"), "").unwrap();
        // dead: heartbeat an hour old
        let dead = root.join("running/20260101_010102_1__dead.job.MAC.2");
        std::fs::write(&dead, "ID=\"dead\"\n").unwrap();
        let hb = root.join("running/20260101_010102_1__dead.job.MAC.2.heartbeat");
        std::fs::write(&hb, "").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        filetime_set(&hb, old);

        let reaped = reap(&r, 20).unwrap();
        assert_eq!(reaped, vec!["dead"], "only the stalled one");
        assert!(root.join("queue/REQUEUED_20260101_010102_1__dead.job").is_file());
        assert!(!hb.exists(), "the stale heartbeat goes with it");
        assert!(root.join("running/20260101_010101_1__live.job.MAC.1").is_file(), "a live render is untouched");
        let _ = std::fs::remove_dir_all(&root);
    }

    // A tiny helper so the reap test can age a file without a new dependency.
    fn filetime_set(p: &Path, t: std::time::SystemTime) {
        let secs = t.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let stamp = format!("{}", secs);
        // `touch -t` needs a formatted date; -d with @epoch is GNU. Use SetFile-free
        // approach: python is always present on macOS.
        let _ = std::process::Command::new("/usr/bin/python3")
            .args(["-c", "import os,sys; t=int(sys.argv[2]); os.utime(sys.argv[1], (t,t))", &p.to_string_lossy(), &stamp])
            .status();
    }

    #[test]
    fn pausing_hides_the_queue_from_workers_and_resuming_puts_it_back() {
        let root = tmp("pause");
        let r = root.to_string_lossy().to_string();
        std::fs::write(root.join("queue/20260101_010101_1__a.job"), "ID=\"a\"\nPROMPT=\"p\"\n").unwrap();
        std::fs::write(root.join("queue/hi/20260101_010102_1__b.job"), "ID=\"b\"\nPROMPT=\"p\"\n").unwrap();

        assert_eq!(pause_queue(&r).unwrap(), 2);
        assert_eq!(board(&r, 10).queued.len(), 0, "a worker's glob can't see queue/hold");
        assert_eq!(held_count(&r), 2);

        assert_eq!(resume_queue(&r).unwrap(), 2);
        let q = board(&r, 10).queued;
        assert_eq!(q.len(), 2);
        assert_eq!(q[0].priority, "high", "the priority lane survives a pause");
        assert_eq!(held_count(&r), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    // --- progress parsing -------------------------------------------------

    #[test]
    fn progress_is_read_from_whatever_the_renderer_prints() {
        assert_eq!(parse_progress("loading\nstep 12/40\n"), (12, 40));
        assert_eq!(parse_progress("denoising 7 of 25\n"), (7, 25));
        assert_eq!(parse_progress(" 45%|####      | 9/20\n"), (45, 100));
        // nothing that looks like progress -> nothing claimed
        assert_eq!(parse_progress("loading model\nwriting mp4\n"), (0, 0));
        // a version string must not be mistaken for progress
        assert_eq!(parse_progress("ffmpeg 8.1.1\n"), (0, 0));
    }

    #[test]
    fn a_log_tail_returns_the_end_of_the_file() {
        let root = tmp("logs");
        let r = root.to_string_lossy().to_string();
        let body: String = (1..=500).map(|i| format!("line {}\n", i)).collect();
        std::fs::write(root.join("logs/clip.MAC.log"), format!("{}step 30/40\n", body)).unwrap();
        let v = log_tail(&r, "clip", "MAC", 50).unwrap();
        let lines = v["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 50);
        assert_eq!(v["step"], 30);
        assert_eq!(v["percent"], 75);
        assert!(log_tail(&r, "nope", "MAC", 50).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    // --- overnight runs ---------------------------------------------------

    #[test]
    fn a_run_turns_a_prompt_list_into_a_whole_night() {
        let root = tmp("plan");
        let r = root.to_string_lossy().to_string();
        let plan = RunPlan {
            run: "overnight".into(),
            prompts: vec!["a roof".into(), "  ".into(), "a gutter".into()],
            sizes: vec!["1080x1920".into(), "1920x1080".into()],
            seeds: 2,
            mode: "hero".into(),
            member: "Aiden".into(),
            ..Default::default()
        };
        let out = plan_run(&r, &plan, "20260728_220000").unwrap();
        assert_eq!(out["queued"], 8, "2 prompts × 2 sizes × 2 seeds (the blank line is dropped)");
        let b = board(&r, 50);
        assert_eq!(b.queued.len(), 8);
        assert!(b.queued.iter().all(|c| c.run == "overnight"));
        assert!(b.queued.iter().all(|c| c.member == "Aiden"), "every job records who asked");
        // prompt-major: an interrupted night still covers both prompts early
        assert_eq!(b.queued[0].prompt, "a roof");
        assert!(b.queued.iter().take(4).all(|c| c.prompt == "a roof"));
        assert!(b.queued.iter().skip(4).all(|c| c.prompt == "a gutter"));
        // and the run is visible with live progress
        let rs = runs(&r);
        assert_eq!(rs.len(), 1);
        assert_eq!(rs[0]["queued"], 8);
        assert_eq!(rs[0]["finished"], false);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_run_refuses_to_be_absurd() {
        let root = tmp("plansafe");
        let r = root.to_string_lossy().to_string();
        assert!(plan_run(&r, &RunPlan { prompts: vec![], ..Default::default() }, "s").is_err());
        let many: Vec<String> = (0..300).map(|i| format!("prompt {}", i)).collect();
        assert!(plan_run(&r, &RunPlan { prompts: many.clone(), ..Default::default() }, "s").is_err(),
            "300 prompts is more than one night");
        let huge = RunPlan { prompts: many[..100].to_vec(), seeds: 12, sizes: vec!["1080x1920".into()], ..Default::default() };
        assert!(plan_run(&r, &huge, "s").is_err(), "1200 jobs must be refused, not queued");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_run_report_adds_up_the_morning_after() {
        let root = tmp("report");
        let r = root.to_string_lossy().to_string();
        for (i, host) in [("a", "MAC1"), ("b", "MAC1"), ("c", "MAC2")] {
            std::fs::write(root.join(format!("done/2026010{}_010101_1__{}.job.{}.9.ok", i.len(), i, host)),
                format!("ID=\"{}\"\nPROMPT=\"p\"\nRUN=\"night\"\n", i)).unwrap();
            std::fs::write(root.join(format!("done/{}.json", i)),
                serde_json::json!({"id":i,"duration_secs":600,"worker":host}).to_string()).unwrap();
        }
        std::fs::write(root.join("failed/20260101_010109_1__d.job.MAC2.9.rc137"),
            "ID=\"d\"\nPROMPT=\"p\"\nRUN=\"night\"\n").unwrap();
        std::fs::write(root.join("queue/20260101_010110_1__e.job"), "ID=\"e\"\nPROMPT=\"p\"\nRUN=\"other\"\n").unwrap();
        write_review(&r, &Review { id: "a".into(), state: "approved".into(), ..Default::default() }).unwrap();

        let rep = run_report(&r, "night");
        assert_eq!(rep["counts"]["done"], 3);
        assert_eq!(rep["counts"]["failed"], 1);
        assert_eq!(rep["counts"]["queued"], 0, "the other run's job isn't counted");
        assert_eq!(rep["counts"]["approved"], 1);
        assert_eq!(rep["render_secs"], 1800);
        assert_eq!(rep["per_host"][0]["host"], "MAC1");
        assert_eq!(rep["per_host"][0]["clips"], 2);
        assert_eq!(rep["finished"], true);
        let _ = std::fs::remove_dir_all(&root);
    }

    // --- autopilot --------------------------------------------------------

    fn pol(retry: u32, streak: u32) -> AutoPolicy {
        AutoPolicy { stale_min: 20, max_retry: retry, fail_streak: streak, member: "Robot".into() }
    }

    #[test]
    fn autopilot_retries_a_failure_once_then_leaves_it_alone() {
        let root = tmp("auto1");
        let r = root.to_string_lossy().to_string();
        std::fs::write(root.join("failed/20260101_010101_1__flaky.job.MAC.9.rc1"),
            "ID=\"flaky\"\nPROMPT=\"p\"\nWIDTH=1080\nHEIGHT=1920\nFRAMES=97\nRUN=\"night\"\n").unwrap();

        let out = autopilot_tick(&r, &pol(1, 99), "20260729_030000");
        assert_eq!(out.retried, vec!["flaky"]);
        let q = board(&r, 10).queued;
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].retry, 1, "the retry count rides along in the job file");
        assert_eq!(q[0].run, "night", "and it stays in its run");
        assert!(!root.join("failed/20260101_010101_1__flaky.job.MAC.9.rc1").exists(),
            "the failed record is moved aside so it can't be retried forever");

        // pretend that retry failed too
        std::fs::write(root.join("failed/20260101_010105_1__flaky.job.MAC.9.rc1"),
            "ID=\"flaky\"\nPROMPT=\"p\"\nRETRY=1\n").unwrap();
        let out = autopilot_tick(&r, &pol(1, 99), "20260729_031000");
        assert!(out.retried.is_empty(), "one retry means one retry");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_memory_kill_is_retried_differently() {
        let root = tmp("auto2");
        let r = root.to_string_lossy().to_string();
        std::fs::write(root.join("failed/20260101_010101_1__heavy.job.MAC.9.rc137"),
            "ID=\"heavy\"\nPROMPT=\"p\"\nWIDTH=1920\nHEIGHT=1080\nFRAMES=97\n").unwrap();

        // no Mac has reported its RAM -> make the job smaller instead
        let out = autopilot_tick(&r, &pol(1, 99), "20260729_030000");
        assert_eq!(out.retried, vec!["heavy"]);
        let c = &board(&r, 10).queued[0];
        assert!(c.width < 1920 && c.height < 1080, "an OOM retry at the same size just dies again");
        assert_eq!(c.width % 8, 0, "dimensions stay a multiple of 8");
        assert_eq!(c.perf, "light");

        // with a 64GB Mac known to the farm, ask for that instead of shrinking
        let root2 = tmp("auto3");
        let r2 = root2.to_string_lossy().to_string();
        std::fs::write(root2.join("running/.worker.BIG.info"), "HOST=\"BIG\"\nRAM_GB=64\nSTATE=\"idle\"\n").unwrap();
        std::fs::write(root2.join("failed/20260101_010101_1__heavy.job.MAC.9.rc137"),
            "ID=\"heavy\"\nPROMPT=\"p\"\nWIDTH=1920\nHEIGHT=1080\nFRAMES=97\n").unwrap();
        autopilot_tick(&r2, &pol(1, 99), "20260729_030000");
        let c = &board(&r2, 10).queued[0];
        assert_eq!(c.min_ram_gb, 64, "it should wait for the big Mac");
        assert_eq!(c.width, 1920, "no need to shrink it if a Mac can afford it");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&root2);
    }

    // The important safety property: a broken setup must not burn the whole night.
    #[test]
    fn a_run_of_failures_stops_the_farm_instead_of_looping() {
        let root = tmp("auto4");
        let r = root.to_string_lossy().to_string();
        for i in 0..3 {
            std::fs::write(root.join(format!("failed/2026010{}_010101_1__bad{}.job.MAC.9.rc2", i + 1, i)),
                format!("ID=\"bad{}\"\nPROMPT=\"p\"\n", i)).unwrap();
        }
        for i in 0..4 {
            std::fs::write(root.join(format!("queue/2026020{}_010101_1__next{}.job", i + 1, i)),
                format!("ID=\"next{}\"\nPROMPT=\"p\"\n", i)).unwrap();
        }
        let out = autopilot_tick(&r, &pol(1, 3), "20260729_030000");
        assert!(out.paused, "three failures in a row with work left means something is broken");
        assert!(out.reason.contains("in a row"));
        assert_eq!(board(&r, 10).queued.len(), 0, "the queue is held, not deleted");
        assert_eq!(held_count(&r), 4);
        assert!(out.retried.is_empty(), "it must not also retry into the same wall");
        // and the jobs come back untouched
        assert_eq!(resume_queue(&r).unwrap(), 4);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_review_marked_retake_is_left_for_the_human() {
        let root = tmp("auto5");
        let r = root.to_string_lossy().to_string();
        std::fs::write(root.join("failed/20260101_010101_1__mine.job.MAC.9.rc1"),
            "ID=\"mine\"\nPROMPT=\"p\"\n").unwrap();
        write_review(&r, &Review { id: "mine".into(), state: "retake".into(), by: "Aiden".into(), ..Default::default() }).unwrap();
        let out = autopilot_tick(&r, &pol(2, 99), "20260729_030000");
        assert!(out.retried.is_empty(), "someone already has plans for that clip");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_one_mac_babysits_the_farm() {
        let root = tmp("super");
        let r = root.to_string_lossy().to_string();
        assert!(claim_supervisor(&r, "MAC1", 100), "first in wins");
        assert!(claim_supervisor(&r, "MAC1", 160), "and keeps it");
        assert!(!claim_supervisor(&r, "MAC2", 160), "a second Mac must not also act");
        // once the holder goes quiet the lock is up for grabs
        filetime_set(&runs_dir(&r).join(".autopilot.lock"),
            std::time::SystemTime::now() - std::time::Duration::from_secs(600));
        assert!(claim_supervisor(&r, "MAC2", 800), "a dead supervisor is replaced");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn autopilot_writes_down_what_it_did() {
        let root = tmp("autolog");
        let r = root.to_string_lossy().to_string();
        log_autopilot(&r, "MAC1", "requeued 2 stalled");
        log_autopilot(&r, "MAC1", "run “night” finished: 12 done, 1 failed");
        let tail = autopilot_log_tail(&r, 10);
        assert_eq!(tail.len(), 2);
        assert!(tail[1].contains("night"));
        assert!(tail[0].contains("MAC1"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unreachable_share_is_reported_not_guessed() {
        let b = board("/nope/not/a/share", 10);
        assert!(!b.reachable);
        assert!(b.queued.is_empty());
    }
}
