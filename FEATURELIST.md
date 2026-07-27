# FEATURELIST — VideoGen Network Switch Load Balancer

Next features to pick up at the office. Grouped by theme, tagged by priority
(**P0** = do next, **P1** = soon, **P2** = nice-to-have) with a rough effort
(S/M/L) and the *why*. Current state: shell farm + provisioning + perf profiles +
test/hero cherry-pick + FarmMon menubar app all shipped.

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

## 3. FarmMon app (the menubar UI)

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

- **P0 · S · Signed + notarised app** — codesign FarmMon with the Apple Developer ID so
  teammates don't hit "unidentified developer". Ship a `.dmg`.
- **P1 · M · One-file installer** — a `.pkg` that drops FarmMon.app, installs the
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

1. **Priority lanes** (§1) — stop heroes waiting behind sweeps.
2. **Heartbeats** (§2) — kill false requeues.
3. **Auto-promote winners + thumbnails in the app** (§1 + §3) — makes the
   test→hero cherry-pick loop actually pleasant.
4. **Enqueue from the app** (§3) — removes the terminal for everyone else.
5. **Sign + notarise** (§5) — clean install for the team.

*Add ideas here as they come up — this is the pickup list.*
