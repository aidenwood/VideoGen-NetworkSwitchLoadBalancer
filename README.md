# LTX Mac Farm — VideoGen Network Switch Load Balancer

Turn a pile of Apple Silicon Macs into a **local AI video render farm**. Wire them
to a cheap gigabit switch, point them at one shared queue, and they chew through
LTX-2.3 (MLX) video jobs **in parallel** — no cloud, no per-render cost.

Built for a marketing team with 4× M4 Macs, but it scales to any number.

> ### ⬇️ Download the menubar app
> **[Get the latest LTX Mac Farm.app (.dmg) →](https://github.com/aidenwood/VideoGen-NetworkSwitchLoadBalancer/releases/latest)**
>
> Grab the `.dmg` from the latest release, open it, drag **LTX Mac Farm** into `/Applications`.
> The app is the live status tray + ping sounds — the render scripts live in this repo.
>
> **First launch (the app isn't from the App Store, so macOS blocks it once):**
> - If you see **"can't be opened / Apple could not verify…"** → **System Settings → Privacy & Security** → scroll down → **Open Anyway**.
> - If macOS says the app is **"damaged"** (stricter on macOS 15/26), run this one line in **Terminal** — it just removes the download flag:
>   ```bash
>   xattr -dr com.apple.quarantine "/Applications/LTX Mac Farm.app"
>   ```
>   Then open it normally. You only ever do this once per Mac.

---

## The one thing to understand first

You **cannot** merge several Macs into one big GPU to render a *single* clip
faster. Splitting one diffusion job across machines means shuffling activation
tensors between them every denoising step — over ethernet/WiFi that's ~1000× too
slow, so it ends up *slower* than one Mac. (That trick only works in datacenters
wired with NVLink at ~900 GB/s.)

What **does** work — and what this repo does — is **job-level parallelism**: give
each Mac a *different* clip. 4 Macs = ~4× throughput, near-linear. Only tiny job
files and finished MP4s cross the network, so a $20 gigabit switch (or even WiFi)
is plenty.

```
                 ┌──────────────────────┐
                 │  Coordinator + share  │  queue of jobs  ─┐  finished MP4s
                 └───────────┬───────────┘                  ▼
        ┌───────────────┬────┴────────┬───────────────┐  done/
   ┌────▼────┐     ┌────▼────┐   ┌────▼────┐     ┌────▼────┐
   │  Mac 1  │     │  Mac 2  │   │  Mac 3  │     │  Mac 4  │  each claims a job,
   │ LTX2-MLX│     │ LTX2-MLX│   │ LTX2-MLX│     │ LTX2-MLX│  renders LOCALLY,
   └─────────┘     └─────────┘   └─────────┘     └─────────┘  writes it back
```

---

# Setup — the whole thing, in order

There are **five stages**. Read the label on each: some run on **ONE** Mac (the
"coordinator" — the Mac that holds the shared folder), the rest run on **EVERY**
Mac that will render.

| Stage | Where | How often | What you get |
|---|---|---|---|
| **1. Coordinator: shared queue** | the ONE coordinator Mac | once | a folder every Mac can see |
| **2. Wire the network** | every Mac | once | the fast private farm LAN |
| **3. Stage the models** | coordinator, then each worker | once (+ when models change) | the AI models on every Mac |
| **4. Install + join** | every worker Mac | once | a Mac that renders |
| **5. The menubar app** | every Mac (optional) | once | live status + ping sounds |

Do Stage 1 first, then 2, then 3, then 4. Stage 5 is polish — skip it and the
farm still works.

---

## Stage 1 — Coordinator: create the shared queue

Pick **one** Mac to be the coordinator. It holds the shared folder every other Mac
reads jobs from. (It can *also* render — it's just the one hosting the share.)

1. On that Mac, make a folder called `RenderFarm` in your home folder
   (`~/RenderFarm`).
2. Open **System Settings → General → Sharing**.
3. Turn **File Sharing** ON.
4. Click the **ⓘ** next to File Sharing → **+** under *Shared Folders* → add your
   `~/RenderFarm` folder.
5. Find this Mac's **name**: System Settings → General → About → **Name** (e.g.
   `mac-studio`). Write it down — every worker needs it as `<name>.local`.

✅ **Done when:** File Sharing is on and `RenderFarm` is in the shared-folders list.

---

## Stage 2 — Wire the network (every Mac)

The farm runs on a private gigabit switch so big file moves never touch office
WiFi. Each Mac uses **two** connections at once:

- **WiFi → office router → internet** — for model downloads + normal use.
- **Ethernet → the gigabit switch → farm LAN** — for the queue + file moves.

Steps, on **each** Mac:

1. Plug the Mac's ethernet into the switch.
   - **MacBook Pro / Air?** Use a **Gigabit** USB-C→Ethernet adapter. (Many cheap
     ones are secretly 10/100 — check the box says *Gigabit* / *1000 Mbps*.)
2. Open **System Settings → Network → (⋯ button) → Set Service Order**.
3. Drag **Wi-Fi ABOVE Ethernet.** This tells macOS: internet goes over WiFi, farm
   traffic goes over the switch — automatically.

The switch does **not** need to reach your office router, and it doesn't matter
what room it's in. Even as an isolated island, Mac `.local` names still resolve
over it (`mac1.local`, etc.), so the share just works.

```
  Wi-Fi   ── office router ── internet         (each Mac, independently)
  Ethernet ─┐
  Ethernet ─┼── gigabit switch                 (isolated farm LAN, .local names)
  Ethernet ─┘
```

✅ **Done when:** Wi-Fi is above Ethernet in Service Order on every Mac, and all
Macs are cabled into the switch.

---

## Stage 3 — Stage the models onto the share

The AI models are big (~60GB) and HuggingFace can be painfully slow (throttled
boxes see <100 KB/s — days for the full model). So you download them **once** on
the coordinator, then every worker pulls them off the fast switch in minutes.
`MANIFEST.txt` is the master list of what everyone must have.

**3a — On the coordinator (once):**

```bash
FARM_ROOT=/Volumes/RenderFarm ./seed_farm_assets.sh
```

This publishes every model + LoRA in `MANIFEST.txt` into
`$FARM_ROOT/{models,loras}` on the share.

> **It does not duplicate the models.** They're ~87GB, and on the coordinator the
> share sits on the same disk as the HuggingFace cache — so copying would burn
> 87GB on files already there. When source and destination share a volume this
> **hardlinks**: the share gets its own directory entries pointing at the same
> data. Measured on the real 87GB set: **0.68s, 67MB used.** Workers reading it
> over SMB can't tell the difference, and it's safe because HF blobs are
> content-addressed and never modified in place.
>
> Across volumes (a real external/NAS share) it falls back to rsync, since
> hardlinks can't span filesystems. `--copy` forces real copies; `--dry-run`
> shows what it would do.

**3b — On each worker:** handled automatically by `setup.command` in Stage 4
(it runs `provision.command` for you). You don't do anything extra here.

**Adding a character LoRA later:** uncomment its line in `MANIFEST.txt` → re-run
`seed_farm_assets.sh` on the coordinator → run `provision.command` on each Mac.

✅ **Done when:** `/Volumes/RenderFarm/models` and `/loras` on the coordinator
contain the files listed in `MANIFEST.txt`.

---

## Stage 4 — Each worker Mac: install + join the farm

Do this on **every** Mac that will render (including the coordinator if you want
it rendering too). This is the two-double-clicks part.

1. **Mount the share.** Finder → **Go → Connect to Server** →
   `smb://<coordinator-name>.local/RenderFarm` → **Connect**. Approve it if macOS
   asks. It mounts at `/Volumes/RenderFarm`.
   *(Replace `<coordinator-name>` with the name from Stage 1, step 5.)*
2. **Point the setup script at the coordinator.** Open `setup.command` in this
   folder (right-click → Open With → TextEdit), change the `COORDINATOR=` line
   near the top to the coordinator's name, save.
3. **Double-click `setup.command`.** It installs the whole toolchain (Homebrew,
   uv, the LTX2-MLX runtime, mflux) and pulls the models off the share.
   **~15–30 min, mostly unattended.** It prints what it's doing as it goes.
4. **Nothing to set for speed or memory.** The worker sizes itself to this Mac's
   RAM (`PERF=auto`) and takes its limits from `farm.conf` on the share. Only
   come back here if this *one* Mac must differ — and even then, prefer a
   `farm.conf.<hostname>` on the share. See
   [Performance profiles](#performance-profiles--full-vs-light) below.
5. **Double-click `start_worker.command`.** The Mac is now in the farm, pulling
   jobs. Leave the window open — closing it stops that worker.

✅ **Done when:** `start_worker.command` prints `worker online` and starts
claiming jobs.

> **Safety:** a Mac won't run two render jobs at once (a lockfile enforces the
> one-GPU-job rule); it pauses instead of crashing if the disk gets too full
> (`MIN_FREE_GB`, default 15GB); and it won't *claim* a job at all while it's
> short on RAM — the job stays queued for a healthier Mac rather than dragging
> that one into swap. Sizing is automatic per machine; see
> [`docs/OOM_LIMITS.md`](docs/OOM_LIMITS.md).

---

## Stage 5 — the app (menubar + browser)

`desktop/` is a native macOS app (**LTX Mac Farm**, Tauri). It lives in the menubar,
and it also **serves the same interface in a browser** so the whole team can use the
farm without touching Terminal or mounting anything.

**Easiest:** download the prebuilt `.dmg` from the
**[latest release](https://github.com/aidenwood/VideoGen-NetworkSwitchLoadBalancer/releases/latest)**,
drag **LTX Mac Farm** into `/Applications`, open it.

**Or build it yourself:**

```bash
cd desktop && npm install && npm run build
# → creates "LTX Mac Farm.app" and a .dmg in
#   desktop/src-tauri/target/release/bundle/
```

Five views, all of them live off the shared folder:

| View | What you get |
|---|---|
| **Setup** | this Mac's remaining steps, each with a button that does it |
| **Farm** | counts, the queue lane, recent pings, runs, and this farm's own numbers |
| **Board** | the queue as a kanban board — see below |
| **Review** | proof stills + finished clips: approve, send back, or render the winner |
| **Team** | who's connected, whose Mac is rendering what, right now |
| **Checks** | ✅/⚠️/❌ per setup step, farm-wide limits, operations, autopilot, gateway |

### The job board

The pipeline as four lanes — **Queued → Rendering → Done → Failed**:

- **Drag to reorder** what renders next. Claim order is filename order on the share, so
  dragging genuinely re-prioritises the farm (it renames the job file) — nothing cosmetic.
- **↑ Priority** drops a job into `queue/hi/`, which every worker scans first.
- **Queue a clip** from the board: prompt, delivery size, hero-vs-proof, seed sweep.
- **Watch or download** a finished clip straight from the browser — no SMB mount needed
  to review a render.
- **Requeue** a failed job (its log is one click away), or **run a finished one again**.
- **Variants…** on any card offers the same shot at the other delivery sizes, a few
  prompt edits (golden hour, storm mood, slow push in…), a 4-seed sweep, or a cheap proof
  still. Tick what you want → the farm renders them next.
- **Search and filter** by prompt, size, Mac, run or review state, **select several cards**
  for bulk work, and drive it from the keyboard (`/` search, `a` select, `p` priority,
  `x` remove, `r` requeue, `1`–`6` views).
- Every card shows **who queued it**, which **run** it belongs to, roughly **how long it
  takes on your farm**, and when it should start — measured from your own finished renders,
  not guessed.
- Image-to-video and LoRA jobs too: **drop an image onto the page** to upload it to
  `assets/`, pick a LoRA off the share, save a setup as a **preset**.

### Plan an overnight run, review it in the morning

Paste a shot list — one prompt per line — pick your delivery sizes and how many takes,
and the board tells you the size of the night *before* you commit it (jobs × roughly how
long across however many Macs are up). Everything gets tagged with one run name.

Choose **proofs** and the night renders cheap stills instead: in the morning, the Review
tab is a contact sheet, and one click on a winner queues its full render. That's the
cherry-pick loop the README describes, without the terminal.

**Autopilot** (off by default, one Mac only — Checks → Overnight autopilot) is what makes
it unattended. It requeues jobs whose Mac died, retries a failure once, and on a memory
kill either asks for a bigger Mac or shrinks the job. If several jobs fail in a row it
**pauses the whole queue** rather than burning the rest of the night, and everything it
does goes to `logs/autopilot.log`. It only ever requeues work — it never deletes a job.

When a run finishes you get a notification with the tally, and **Report** on the Farm view
opens the morning digest: what landed, what failed (as cards you can act on), who rendered
what, and what's been approved.

### Run the farm without Terminal

The Checks tab also has:

- **Reap** — requeue jobs whose worker died (what `farm_status.sh --reap` does).
- **Pause / Resume** — hold every waiting job; anything mid-render finishes normally.
- **Farm-wide limits** — edit `farm.conf` on the share, so every Mac picks the change up
  within one poll. Values are validated before they're written (that file is `source`d by
  bash on every worker).
- **This farm's numbers** — clips per Mac, average time per delivery size, and how many
  renders peaked above their memory budget. That last number is what to tune
  `MEM_BUDGET_PCT` against instead of guessing.

### The web gateway (open it in a browser)

Every copy of the app serves its UI over HTTP. The tray menu has both links:

```
Open in browser    http://127.0.0.1:8787/?k=<key>       ← this Mac
Copy team link     http://<host>.local:8787/?k=<key>    ← anyone on the office LAN
```

It opens automatically when the app launches (turn that off in Settings → Web gateway),
and both surfaces share one live state — change a setting in the menubar and an open
browser tab follows within ~2 seconds.

⚠️ **Before you turn on LAN sharing, read this.** Sharing over the network means anyone
who has that link can queue renders and run setup steps on that Mac. It's off by default:
until you tick it, the gateway only answers on the Mac itself. Only enable it on an office
network you trust, share the link with your team directly, and don't post it anywhere
public or forward the port to the internet.

On a phone, the browser view can be added to the home screen, and the bell in the header
turns on a notification as each render lands.

For a Mac nobody sits at (a render node in a cupboard), run the gateway on its own:

```bash
"/Applications/LTX Mac Farm.app/Contents/MacOS/ltx-mac-farm" --serve
```

Details in [`desktop/README.md`](desktop/README.md).

---

# Rendering — queue work & watch it

Once workers are running (Stage 4), queue jobs from **any** Mac with `enqueue.sh`.
It asks, at prompt time, whether to run a cheap **test** proof still first or go
straight to the full **hero** video:

```bash
FARM_ROOT=/Volumes/RenderFarm ./enqueue.sh \
  --id hail_hero --prompt "storm clouds over a roof, cinematic" --seed 8804
# > How should the farm run "hail_hero"?
# >   1) test  — quick z-image still(s) to cherry-pick first (cheap, seconds)
# >   2) hero  — straight to full 1080x1920 video render
```

More examples:

```bash
# image-to-video (still already in /Volumes/RenderFarm/assets/)
./enqueue.sh --id milk --image person.png --prompt "he lifts the glass" --seed 8804

# character LoRA -> still -> i2v, all on the farm
./enqueue.sh --id taste --lora Elijah_lora.safetensors \
  --still-prompt "eljhwd man tasting hail in a kitchen, photoreal" \
  --prompt "he sniffs then licks, cinematic" --seed 8804

# 12-seed sweep — farm splits all 12 across the Macs
./enqueue.sh --id dragon --sweep 12 --prompt "black dragon on a rooftop at dusk"

# force test-proof or hero non-interactively
./enqueue.sh --id x --test --prompt "..."     # cheap stills to cherry-pick
./enqueue.sh --id x --hero --prompt "..."     # straight to video

# JUMP THE QUEUE — a rush hero shot skips the sweep backlog
./enqueue.sh --id rush --priority high --hero --prompt "..."
```

Finished MP4s → `/Volumes/RenderFarm/done/` (each with a `.json` sidecar recording
prompt/seed/model/worker/render-time). Test proofs → `done/proofs/`.

### The cherry-pick loop (spend renders only on winners)

1. Enqueue a sweep in `--test` mode → the farm makes cheap proof stills for every
   seed in `done/proofs/`.
2. Eyeball the stills.
3. Promote the good ones to full video with **`promote.sh`** — it re-queues those
   exact seeds as `--hero`, no hand-copying:

```bash
./promote.sh                          # interactive: pick winners from a list
./promote.sh --seeds "1003 1007" --prompt "..." --priority high   # scripted
./promote.sh --all --prompt "..."     # promote every proof
./promote.sh --dry-run                # show what it would queue, queue nothing
```

### Monitor / recover (on the coordinator)

```bash
./farm_status.sh          # counts (incl. hi-queue) + who's rendering what
./farm_status.sh --reap   # requeue jobs from a Mac that crashed mid-render
```

The reaper is **heartbeat-aware**: a live worker touches its job every 30s, so a
legitimately long hero render is never mistaken for a dead one — only a worker
that actually stopped gets its job requeued.

---

## Performance profiles — `full` vs `light`

| Profile | Flags | Use on |
|---|---|---|
| **auto** | picks one of the below from installed RAM | **the default — leave it** |
| **full** | no `--low-ram`, no tiling, `nice -5` | **64GB Macs only** |
| **light** | `--low-ram --tile-frames 2`, `nice -15` | everything smaller |

`auto` reads each Mac's own `hw.memsize`, so you never have to remember which
machine is which.

> ### ⚠️ The old numbers in this table were wrong
> This table used to claim `full` peaks ~10–16GB and `light` stays in "low
> single-digit GB". **Both were wrong by 3–5×.** One 896×1216 z-image still
> *with* `--low-ram` measured **27.5GB**. That is why `full` is now 64GB-only.
>
> It also means memory can't be managed by watching free RAM: MLX allocations
> are invisible to `ps` (that 27.5GB job reported 4.9GB RSS). Instead each
> worker **prices a job before claiming it** against a 90%-of-RAM budget, and
> releases anything it can't afford back to the queue for a bigger Mac.
>
> Full story: [`docs/MEMORY-INCIDENT-2026-07-28.md`](docs/MEMORY-INCIDENT-2026-07-28.md).

### Which Mac gets which job

Automatic — nobody routes anything by hand:

| Job | Price | 32GB Mac | 64GB Mac |
|---|---|---|---|
| t2v 768×1280 f97 | 25GB | ✅ | ✅ |
| 896×1216 still / `lora_i2v` | 30GB | ❌ | ✅ |
| hero t2v 1080×1920 f97 | 49GB | ❌ | ✅ |

A worker that can't afford a job logs `↩︎ released …` and puts it straight back
for someone bigger. Force it with `./enqueue.sh --min-ram 64`.

> **🚩 Worth knowing:** on today's coefficients *all* 1080×1920 work lands on the
> single 64GB Mac. The video price is **extrapolated, not measured** — it may be
> too pessimistic. Run `./measure_peak.sh` on the 64GB Mac to get the real
> number; if it's lower you get the other three Macs back for hero renders.

### Changing limits across every Mac at once

All memory limits live in **one file on the share**, `$FARM_ROOT/farm.conf`.
Workers re-read it every poll, so an edit on the coordinator reaches the whole
farm in ~15s — no restarts, nobody walking between desks:

```bash
vi /Volumes/RenderFarm/farm.conf     # e.g. VIDEO_GB_PER_MP after measuring
./farm_status.sh                     # watch the BUDGET column move
```

Need one Mac to differ? Add `farm.conf.<hostname>` beside it with only the lines
that change. Overriding in that Mac's `start_worker.command` also works, but it
takes the machine out of farm-wide control — prefer the host file.

📄 **Full detail: [`docs/OOM_LIMITS.md`](docs/OOM_LIMITS.md)** — budgets, the
pricing model, what each log line means, and every tunable.

### Changing limits across every Mac at once

All memory limits live in **one file on the share**, `$FARM_ROOT/farm.conf`.
Workers re-read it every poll, so an edit on the coordinator reaches the whole
farm in ~15s — no restarts, nobody walking between desks:

```bash
vi /Volumes/RenderFarm/farm.conf     # e.g. MEM_BUDGET_PCT, or a measured coefficient
./farm_status.sh                     # watch the BUDGET column move
```

Need one Mac to differ? Add `farm.conf.<hostname>` beside it with only the lines
that change. Overriding in that Mac's `start_worker.command` also works, but it
takes the machine out of farm-wide control — prefer the host file.

📄 **Full detail: [`docs/OOM_LIMITS.md`](docs/OOM_LIMITS.md)** — the per-RAM
budget table, what jetsam does, the five layers of protection, how to read
`peak_mem_gb` out of the job sidecars, and every tunable.

---

## How claiming works (no server, no collisions)

A worker claims a job with an atomic `mv queue/x.job → running/x.job.<host>`. Only
one worker wins the `mv`; the rest grab the next job. No locks, no database, no
duplicate renders. High-priority jobs live in `queue/hi/` and are scanned first. A
crashed worker leaves its job in `running/`; `--reap` requeues anything whose
heartbeat has gone stale.

---

## Requirements

- Apple Silicon Macs (M1 or newer). Current fleet: 3 × 32GB + 1 × 64GB.
  **Under 32GB is not viable** for hero renders — see `docs/OOM_LIMITS.md`.
- macOS with File Sharing (SMB) — built in.
- A gigabit switch + ethernet to each Mac (WiFi works, just slower for file moves).
- Per Mac: enough free disk for the models (~60GB) + renders.
- Installed by `setup.command`: Homebrew, uv, [LTX2-MLX](https://github.com/dgrauet/ltx-2-mlx),
  mflux (z-image, for stills/LoRA).

## Files

| File | What |
|---|---|
| `setup.command` | **fresh Mac, once:** installs the whole toolchain + provisions |
| `seed_farm_assets.sh` | **coordinator, once:** stage models+LoRAs onto the share |
| `provision.command` | **each worker:** pull models+LoRAs from the share |
| `MANIFEST.txt` | source of truth for which models + LoRAs everyone needs |
| `start_worker.command` | double-click on each Mac to join the farm |
| `farm_worker.sh` | the claim-render-repeat loop (heartbeats, disk + RAM guards, one-job lock) |
| `farm.conf` | **farm-wide limits — edit this one file on the share, every Mac follows** |
| `farm_mem.sh` | RAM detection, the 90% budget, job pricing, OOM classification |
| `measure_peak.sh` | measure the real memory curve on a Mac and print the coefficients to paste into `farm.conf` |
| `farm_sitecustomize/` | applies the memory budget inside the render process; reports real peak |
| `enqueue.sh` | add jobs / seed sweeps; `--priority high` to jump the queue |
| `promote.sh` | promote cherry-picked test proofs to full hero renders |
| `farm_status.sh` | counts, in-flight view, per-worker memory, heartbeat-aware `--reap` |
| `job.sample` | the job file format |
| `desktop/test/ui.test.js` | 56 UI behaviour tests — stubs the Tauri bridge and drives the real `index.html`. `npm test` in `desktop/` runs it plus the Rust unit tests |
| `--selftest` | `"/Applications/LTX Mac Farm.app/Contents/MacOS/ltx-mac-farm" --selftest` — checks every wizard action for both roles without clicking anything |
| `docs/OOM_LIMITS.md` | how much memory a render may use, and what happens when it asks for more |
| `docs/MEMORY-INCIDENT-2026-07-28.md` | the measured evidence behind those limits |
| `FEATURELIST.md` | the roadmap / pickup list |

## How hard is this really?

Honest take: **the farm scripts are trivial; the ML toolchain install is the only
real friction** — and `setup.command` automates that.

- **Green path (all Macs identical, share reachable):** Stage 4 is double-click
  `setup.command`, wait, double-click `start_worker.command`. Two clicks + a wait.
- **What can trip you up:** approving the SMB mount in Finder the first time; a
  MacBook Pro needing a Gigabit adapter; `setup.command` needing internet (that's
  why the Macs keep WiFi); and disk space for the models. All called out in
  `setup.command`'s output as it runs.

No terminal knowledge needed beyond double-clicking `.command` files and, to queue
jobs, copying an `enqueue.sh` line.

---

*MIT-licensed. LTX-2.3, MLX, and mflux are separate upstream projects under their
own licenses.*
