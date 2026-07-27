# LTX Mac Farm — VideoGen Network Switch Load Balancer

Turn a pile of Apple Silicon Macs into a **local AI video render farm**. Wire them
to a cheap gigabit switch, point them at one shared queue, and they chew through
LTX-2.3 (MLX) video jobs **in parallel** — no cloud, no per-render cost.

Built for a marketing team with 4× M4 Macs, but it scales to any number.

> ### ⬇️ Download the menubar app
> **[Get the latest LTX Mac Farm.app (.dmg) →](https://github.com/aidenwood/VideoGen-NetworkSwitchLoadBalancer/releases/latest)**
>
> Grab the `.dmg` from the latest release, drag **LTX Mac Farm** into `/Applications`, done.
> The app is the live status tray + ping sounds — the render scripts live in this repo.
> First launch on an unsigned build: right-click the app → **Open** → **Open** to get past Gatekeeper.

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

This copies every model + LoRA in `MANIFEST.txt` into
`/Volumes/RenderFarm/{models,loras}` on the share.

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
4. **Set this Mac's speed profile.** Open `start_worker.command`, set
   `export PERF=full` (a spare/dedicated Mac) or `export PERF=light` (someone's
   daily-driver — stays usable while rendering). See
   [Performance profiles](#performance-profiles--full-vs-light) below.
5. **Double-click `start_worker.command`.** The Mac is now in the farm, pulling
   jobs. Leave the window open — closing it stops that worker.

✅ **Done when:** `start_worker.command` prints `worker online` and starts
claiming jobs.

> **Safety:** a Mac won't run two render jobs at once (a lockfile enforces the
> one-GPU-job rule), and it pauses instead of crashing if the disk gets too full
> (`MIN_FREE_GB`, default 15GB).

---

## Stage 5 — (optional) the menubar app

`desktop/` is a native macOS menubar app (**LTX Mac Farm**, Tauri) that watches the
share and gives everyone **live tray status + a ping sound** each time a job is
dispatched, picked up, or finished — so you know the farm's working without
staring at a terminal.

**Easiest:** download the prebuilt `.dmg` from the
**[latest release](https://github.com/aidenwood/VideoGen-NetworkSwitchLoadBalancer/releases/latest)**,
drag **LTX Mac Farm** into `/Applications`, open it.

**Or build it yourself:**

```bash
cd desktop && npm install && npm run tauri build
# → creates "LTX Mac Farm.app" and a .dmg in
#   desktop/src-tauri/target/release/bundle/
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

Set per Mac in `start_worker.command` (`export PERF=...`), or override per job
with `--perf`:

| Profile | Flags | Use on | Feel |
|---|---|---|---|
| **full** | no `--low-ram`, no tiling, `nice -5` | a Mac **dedicated** to rendering | fastest, peaks ~10–16GB |
| **light** | `--low-ram --tile-frames 2`, `nice -15` | someone's **daily-driver** Mac | slower, peak stays low single-digit GB — Mac stays usable |

So a laptop someone works on all day runs `light` and still chips in without
lagging them; spare Macs run `full` and do the heavy lifting.

> Note: LTX has no literal "use only N GB" flag. `light` uses the real levers it
> exposes (`--low-ram` + temporal tiling), which keep peak well under 16GB.

---

## How claiming works (no server, no collisions)

A worker claims a job with an atomic `mv queue/x.job → running/x.job.<host>`. Only
one worker wins the `mv`; the rest grab the next job. No locks, no database, no
duplicate renders. High-priority jobs live in `queue/hi/` and are scanned first. A
crashed worker leaves its job in `running/`; `--reap` requeues anything whose
heartbeat has gone stale.

---

## Requirements

- Apple Silicon Macs (M1 or newer; built/tested on M4 Max, 36GB).
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
| `start_worker.command` | double-click on each Mac to join the farm (set `PERF` here) |
| `farm_worker.sh` | the claim-render-repeat loop (heartbeats, disk guard, one-job lock) |
| `enqueue.sh` | add jobs / seed sweeps; `--priority high` to jump the queue |
| `promote.sh` | promote cherry-picked test proofs to full hero renders |
| `farm_status.sh` | counts, in-flight view, heartbeat-aware `--reap` for crashed jobs |
| `job.sample` | the job file format |
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
