# FEATURELIST — VideoGen Network Switch Load Balancer

Next features to pick up at the office. Grouped by theme, tagged by priority
(**P0** = do next, **P1** = soon, **P2** = nice-to-have) with a rough effort
(S/M/L) and the *why*. Current state: shell farm + provisioning + perf profiles +
test/hero cherry-pick + LTX Mac Farm menubar app all shipped.

---

## 1. Orchestration & scheduling

- **P0 · S · Priority lanes** — `--priority high|normal` writes to `queue/hi/` scanned
  first, so a hero shot jumps the sweep backlog instead of waiting behind 200 seeds.
- ~~**P0 · M · Auto-promote winners**~~ — **shipped** as the app's Review tab: the proof
  stills are a contact sheet and *Render hero* re-enqueues that exact seed at full size.
  `promote.sh` still exists for the terminal path.
- **P1 · M · Dependency chains** — a job can declare `NEEDS=<id>` so multi-shot
  sequences (still → i2v → upscale) run in order across the farm.
- **P1 · S · Fair scheduling** — round-robin per requester so one person's 200-seed
  sweep doesn't starve everyone else's single clips.
- ~~**P2 · M · Cost/time estimator**~~ — **shipped**: per-card estimates and queue ETA come
  from `done/*.json` (per size + frames + mode), simulated across the Macs that are up, and
  the overnight planner prices a whole night before you commit it.

## 2. Reliability & scale

- **P0 · S · Heartbeats** — each worker touches `running/<job>.heartbeat` every 30s;
  `--reap` uses real staleness instead of a flat 45-min guess. Kills false requeues on
  legitimately long hero renders.
- ~~**P1 · M · Auto-reaper daemon**~~ — **shipped** as the app's autopilot (one Mac holds a
  heartbeat lock on the share and reaps every minute). A LaunchAgent is still the answer for
  a Mac that shouldn't run the app at all — or run it with `--serve`.
- **P1 · S · Per-worker concurrency guard** — a lockfile so a double-launched
  `start_worker` can't run two GPU jobs on one Mac (violates the hard rule).
- **P1 · M · Cloud burst** — when the local queue depth > threshold, spill overflow
  jobs to `wan_cloud_export.py` (fal.ai H100) automatically. Local for volume, cloud
  for spikes.
- **P2 · S · Disk guard** — worker refuses a job if free space < model + output headroom,
  instead of OOM/ENOSPC mid-render.
- ~~**P0 · L · RAM-aware admission control + farm-wide OOM limits**~~ — **shipped**,
  implementing `docs/MEMORY-INCIDENT-2026-07-28.md` §4–5. Workers detect `hw.memsize`,
  budget to 90%, **price each job before claiming it** and release anything they can't
  afford back to the queue for a bigger Mac; `MIN_RAM_GB` (`enqueue.sh --min-ram`) pins
  a job to big machines; rc=137 requeues with a raised floor and a 90s drain instead of
  dying in `failed/`; `PERF=auto` gates `full` to 64GB. All of it configured from ONE
  file on the share (`farm.conf`), reloaded every poll. Also corrected the profile peak
  numbers, which were 3–5× too low, and fixed a bash 3.2 `set -u` empty-array bug that
  was killing every `t2v` job before it started. See `docs/OOM_LIMITS.md`.
- **P0 · S · Measure the video memory curve** — `./measure_peak.sh` on the 64GB Mac.
  The video coefficient is still **extrapolated**, which prices hero 1080×1920 at ~49GB
  and reserves it for the one 64GB machine. If the real number is lower, that guess is
  costing three Macs' worth of hero throughput. The script fits the curve and prints the
  `farm.conf` lines to paste.
- **P1 · S · Feed measured peaks back into pricing** — every sidecar now records
  `peak_mem_gb`; refit the coefficients from `done/*.json` periodically instead of
  trusting a static estimate. *(The app now surfaces the "renders over their memory budget"
  count from those sidecars, which is the signal to refit — the refit itself is still manual.)*

## 3. LTX Mac Farm app (the menubar UI)

> ### ✅ Shipped
> - **Settings / Connection page** — the Setup + Checks views: role-aware guided setup,
>   live per-step ✅/⚠️/❌, persisted config (JSON in app data, hot-reloaded by the watcher),
>   mount helper, tray entry. `--selftest` drives every path headlessly.
> - **Enqueue from the app** — the Board's composer (prompt, name, delivery size,
>   hero-vs-proof, seed sweep, priority). Written natively, byte-compatible with
>   `enqueue.sh`, prompts escape-hardened.
> - **Per-worker rows** — the Team view: person, Mac model, RAM, role, profile, current job
>   + elapsed, finished count, memory pressure/swap, and flags for *no worker running* /
>   *app not running*. Fed by `presence/<host>.json` + `.worker.<HOST>.info` + heartbeats.
> - **Web gateway** — the same UI served over HTTP (127.0.0.1 by default, LAN opt-in with a
>   32-hex key), auto-opened on launch, plus `--serve` for headless Macs. One
>   `Core::dispatch` behind both surfaces, so neither can drift.
> - **Job board** — kanban lanes with drag-to-reorder (renames the job file, because claim
>   order *is* filename order), priority lane, cancel, requeue, run-again, in-browser
>   playback/download of finished clips, and log viewing on failures.
> - **Variant recommendations** — other delivery sizes, prompt edits, seed sweeps and proof
>   stills offered per card; ticking them queues complete jobs.
> - **Thumbnail previews** — poster frames (ffmpeg, cached in `done/.thumbs/`) on the board,
>   and a proof-still contact sheet in the Review tab with one-click *Render hero*: the
>   cherry-pick loop, in the browser.
> - **Review states** — approve / needs-another-take per clip in `reviews/<ID>.json`;
>   autopilot never auto-retries a clip a human has claimed.
> - **Board at scale** — search, filters (size / Mac / run / review), multi-select bulk
>   actions, keyboard control, and per-card estimates + queue ETA from this farm's own
>   sidecar history.
> - **Overnight runs** — paste a shot list → N prompts × sizes × takes queued as one named
>   run, prompt-major, with the size of the night shown before committing; morning report
>   per run (what landed, what failed, who rendered what, what's approved).
> - **Autopilot** — one Mac babysits: reaps stalled jobs, retries failures, handles OOM by
>   asking for a bigger Mac or shrinking the job, pauses the queue on a failure streak,
>   logs everything to `logs/autopilot.log`.
> - **Ops without Terminal** — reap, pause/resume (via `queue/hold/`), and a validated
>   `farm.conf` editor that changes every Mac at once.
> - **Stats** — clips per Mac, average by delivery size, renders over their memory budget.
> - **i2v + LoRA + uploads + presets** — drop an image to upload into `assets/`, pick LoRAs
>   off the share, save composer setups.
> - **Phone** — installable web manifest + icon, and opt-in browser notifications.
> - **React port** — the UI is React 19 + TypeScript + Vite (`desktop/ui-react`), embedded
>   into the binary by `build.rs` and served to both surfaces. The command surface is typed
>   end to end: `commands.ts` is checked against Rust's `COMMANDS` by a cargo test and by
>   `--selftest`, so a dead button is a compile error or a failed test rather than a
>   support call. The 223-check behaviour suite was kept green through the port by
>   preserving every DOM id — the port is verified behaviour-preserving, not just
>   "looks the same".
>
> Still open below: sound preferences, tray mini-stats, auto-assembly.

- **P0 · M · Settings / Connection page** — a "Setup & Verify" view in the app so nobody
  reverse-engineers the network from the README. Two halves:
  - **What to connect (the checklist):** show, per Mac, the connections this box needs —
    (1) ethernet → the gigabit switch, (2) WiFi above Ethernet in Service Order, (3) the
    SMB share mounted at the configured path. Each row is a live ✅/❌, not static text.
  - **Verify the link (one button):** a `verify_link` Tauri command that runs the checks
    and returns a structured report the UI renders green/red with the exact fix per failure:
    1. share path exists + is a mount (`FARM_ROOT` reachable, is it actually SMB not a stray
       local dir) — today `farm_root()` in `lib.rs:48` only reads the env var; make it read
       persisted config first, env second, default third.
    2. the four queue dirs (`queue/ running/ done/ failed/` + `queue/hi/`) exist & are
       writable (touch a `.probe` file and delete it — proves write perms over SMB).
    3. coordinator reachable by name — `ping -c1 <coordinator>.local` and/or list the share.
    4. workers seen — read `running/*.heartbeat` mtimes (the new heartbeat files) to show
       which hosts are alive *right now*, distinct from just "a job is in running/".
  - **Editable config, persisted:** coordinator name + share path (derive
    `smb://<name>.local/RenderFarm`), PERF default, `MIN_FREE_GB`. Persist via
    `tauri-plugin-store` (JSON in app data) so it survives relaunch — replaces the
    env-var-only `FARM_ROOT`. `spawn_watcher` (`lib.rs:124`) should read from the store and
    hot-reload when it changes.
  - **Mount helper:** a "Mount share" button that shells `open "smb://<name>.local/RenderFarm"`
    (Finder handles the auth prompt) so a new teammate never touches Connect-to-Server.
  - Wire it into the tray menu ("Setup & Verify…") and as a tab in `ui/index.html` beside the
    live dashboard. New commands to add to the `invoke_handler` (`lib.rs:234`): `verify_link`,
    `get_config`, `save_config`, `mount_share`.

- **P0 · M · Thumbnail previews** — dashboard shows the finished MP4's poster frame +
  click-to-play, and proof stills as a contact-sheet grid. Turns it from a counter into
  a review surface.
- **P0 · S · Enqueue from the app** — a small "New render" form (prompt, seed/sweep,
  test|hero, LoRA picker) so nobody touches the terminal to queue work.
- **P1 · S · Per-worker rows** — show each Mac by name: online/offline, current job,
  clips/hour, this-session count. Instantly see if `mac3` fell off the switch.
- **P1 · S · Sound preferences** — per-event sound picker + mute/Do-Not-Disturb window
  so the office isn't a slot machine during a 200-clip sweep.
- **P1 · M · Menubar mini-stats** — live count in the tray title (e.g. `▶3 ✓18`) without
  opening the dashboard.
- **P2 · M · Windows/Linux workers** — the app is Tauri (cross-platform); a PC on the
  switch could at least monitor, and render if a non-MLX backend is added.

## 4. Pipeline quality & outputs

- **P1 · M · Auto-assembly** — when a job group finishes, stitch the shots into one cut
  (ffmpeg) and drop a `done/reels/<group>.mp4`, optionally handing off to ButterCut.
- **P1 · M · Upscale/interp stage** — optional post pass (Real-ESRGAN / frame interp) as
  its own job type so heroes finish at 4K/60 without bloating the gen step.
- **P1 · S · Auto-grade hook** — run the existing `color-grade-ai` LUT on finished clips
  for a consistent look across machines.
- **P2 · S · Metadata sidecars** — write a `.json` next to each MP4 (prompt, seed, model,
  LoRA, worker, render time) for reproducibility + a searchable render log.
- **P2 · M · Web gallery** — a static page served off the coordinator listing every
  render with its prompt/seed, so the team can browse + reuse settings.

## 5. Distribution & onboarding

- **P0 · S · Signed + notarised app** — codesign LTX Mac Farm with the Apple Developer ID so
  teammates don't hit "unidentified developer". Ship a `.dmg`.
- **P1 · M · One-file installer** — a `.pkg` that drops LTX Mac Farm.app, installs the
  toolchain, mounts the share, and provisions — replacing the setup.command sequence.
- **P2 · S · `.env`-driven config** — a single `farm.env` (share path, coordinator name,
  perf default) all scripts + the app read, so a new Mac is one file to edit.

## 6. Observability

- **P1 · S · Throughput log** — append each finished job's render time to a CSV; a weekly
  "clips/day, avg render time, farm utilisation %" summary. Proves the ROI vs cloud.
- **P2 · M · Slack ping** — post batch-complete summaries to `#aidxn-claude` (paths +
  counts), matching the standing done-ping rule. Videos stay local.

---

## Suggested first sprint (highest ROI, low effort)

> ✅ Shipped v0.1.x: Priority lanes, Heartbeats + heartbeat-aware reaper, Disk guard,
> one-GPU-job lock, Metadata sidecars, `promote.sh` auto-promote.

**Next up:**

1. **Settings / Connection page** (§3) — in-app "Setup & Verify" so a new Mac joins
   without the README: live connection checklist + one-button `verify_link`. ← wanted next.
2. **Thumbnail previews** (§3) — turn the dashboard from a counter into a review surface.
3. **Enqueue from the app** (§3) — removes the terminal for everyone else.
4. **Sign + notarise** (§5) — clean install for the team (currently ad-hoc signed).

*Add ideas here as they come up — this is the pickup list.*
