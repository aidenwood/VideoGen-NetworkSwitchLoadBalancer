# FEATURELIST — VideoGen Network Switch Load Balancer

Next features to pick up at the office. Grouped by theme, tagged by priority
(**P0** = do next, **P1** = soon, **P2** = nice-to-have) with a rough effort
(S/M/L) and the *why*. Current state: shell farm + provisioning + perf profiles +
test/hero cherry-pick + LTX Mac Farm menubar app all shipped.

---

## 1. Orchestration & scheduling

- **P0 · S · Priority lanes** — `--priority high|normal` writes to `queue/hi/` scanned
  first, so a hero shot jumps the sweep backlog instead of waiting behind 200 seeds.
- **P0 · M · Auto-promote winners** — after a `--test` sweep, a tiny picker (`promote.sh`
  or a dashboard grid) lets you click the good proof stills; it re-enqueues exactly
  those seeds as `--hero`. Closes the cherry-pick loop without hand-copying seeds.
- **P1 · M · Dependency chains** — a job can declare `NEEDS=<id>` so multi-shot
  sequences (still → i2v → upscale) run in order across the farm.
- **P1 · S · Fair scheduling** — round-robin per requester so one person's 200-seed
  sweep doesn't starve everyone else's single clips.
- **P2 · M · Cost/time estimator** — predict wall-clock for a queue given N workers +
  measured per-shot times; show "ETA 42 min" before you commit a batch.

## 2. Reliability & scale

- **P0 · S · Heartbeats** — each worker touches `running/<job>.heartbeat` every 30s;
  `--reap` uses real staleness instead of a flat 45-min guess. Kills false requeues on
  legitimately long hero renders.
- **P1 · M · Auto-reaper daemon** — fold reaping into a coordinator LaunchAgent so a
  crashed Mac's job returns to the queue with no human running `--reap`.
- **P1 · S · Per-worker concurrency guard** — a lockfile so a double-launched
  `start_worker` can't run two GPU jobs on one Mac (violates the hard rule).
- **P1 · M · Cloud burst** — when the local queue depth > threshold, spill overflow
  jobs to `wan_cloud_export.py` (fal.ai H100) automatically. Local for volume, cloud
  for spikes.
- **P2 · S · Disk guard** — worker refuses a job if free space < model + output headroom,
  instead of OOM/ENOSPC mid-render.

## 3. LTX Mac Farm app (the menubar UI)

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
