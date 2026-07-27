# VideoGen — Network Switch Load Balancer

Turn a pile of Apple Silicon Macs into a **local AI video render farm**. Wire them
to a cheap gigabit switch, point them at one shared queue, and they chew through
LTX-2.3 (MLX) video jobs **in parallel** — no cloud, no per-render cost.

Built for a marketing team with 4× M4 Macs, but it scales to any number.

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

## Quick start for a new teammate (no help needed)

You need: an Apple Silicon Mac, this folder, and the coordinator's share name.

1. **Cable** your Mac's ethernet into the switch. (MacBook Pro? Use a *Gigabit*
   USB-C→Ethernet adapter — many cheap ones are secretly 10/100.)
2. Open **`setup.command`**, change `COORDINATOR` near the top to the host Mac's
   name, then **double-click it**. It installs everything (Homebrew, uv, the
   LTX2-MLX runtime, mflux) and pulls the models off the share. ~15–30 min,
   mostly unattended.
3. **Double-click `start_worker.command`.** You're now rendering.

That's it. Everything after step 2 is double-clicks. See
[How hard is this really?](#how-hard-is-this-really) for the honest breakdown.

---

## Network topology

Each Mac uses **two** connections at once — the clean setup:

- **WiFi → office router → internet** (model downloads, general use)
- **Ethernet → the gigabit switch → private farm LAN** (queue + big file moves)

The switch does **not** need to reach the router, and it doesn't matter what room
it's in. As an island it uses link-local addressing, and **Bonjour `.local` names
still resolve** over it (`mac1.local` etc.), so the file share just works. Render
files stay on the dedicated switch and never clog office WiFi.

> macOS setting: System Settings → Network → (⋯) → **Set Service Order** → keep
> **Wi-Fi above Ethernet**. Internet routes via WiFi, farm traffic via the switch,
> automatically.

```
  Wi-Fi   ── office router ── internet         (each Mac, independently)
  Ethernet ─┐
  Ethernet ─┼── gigabit switch                 (isolated farm LAN, .local names)
  Ethernet ─┘
```

---

## Coordinator setup (one time)

1. Pick any Mac as the **coordinator**. It hosts the shared folder.
2. Make a folder, e.g. `~/RenderFarm`.
3. System Settings → General → Sharing → **File Sharing** on → add `~/RenderFarm`.
4. Note the coordinator's **Name** (System Settings → General → About) — used as
   `<name>.local`.
5. Workers mount it: Finder → Go → Connect to Server →
   `smb://<coordinator-name>.local/RenderFarm` → mounts at `/Volumes/RenderFarm`.

### Provision the models + LoRAs once, share to all

HuggingFace can be painfully slow (throttled boxes see <100 KB/s — the 56GB LTX
model would take *days*). So stage everything **once** and let workers pull it
over the fast switch in minutes. `MANIFEST.txt` is the source of truth for what
everyone must have.

- **Coordinator:** `FARM_ROOT=/Volumes/RenderFarm ./seed_farm_assets.sh`
  → copies the models + LoRAs in `MANIFEST.txt` to `/Volumes/RenderFarm/{models,loras}`.
- **Each worker:** `setup.command` (or `provision.command`) pulls them into the
  local HF cache + `~/farm-loras`. Idempotent — re-run when the manifest changes.

Add a character: uncomment its line in `MANIFEST.txt`, re-run `seed_farm_assets.sh`,
then `provision.command` on each Mac.

---

## Rendering

**Start a worker** on each Mac: double-click **`start_worker.command`** (set its
`PERF` profile near the top). It loops, pulling jobs. Leave it running.

**Queue work** from any node with `enqueue.sh`. It asks, at prompt time, whether
to run a cheap **test** proof first or go straight to the **hero** render:

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
```

Finished MP4s → `/Volumes/RenderFarm/done/`. Test proofs → `done/proofs/`.

**Cherry-pick loop:** enqueue a sweep in `--test` mode → eyeball the stills in
`done/proofs/` → re-enqueue only the winning seeds as `--hero`. You spend the
expensive video renders only on shots you've already approved.

**Monitor / recover** (on the coordinator):

```bash
./farm_status.sh          # counts + who's rendering what
./farm_status.sh --reap   # requeue jobs from a Mac that crashed mid-render
```

---

## Menubar app (optional) — see status + hear the pings

`desktop/` is a native macOS menubar app (**FarmMon**, Tauri) that watches the share
and gives every teammate **live tray status + a notification sound** each time a job
is dispatched, picked up, or finished — so you know the farm's working without
watching a terminal. Build it with `cd desktop && npm install && npm run tauri build`,
then drop `FarmMon.app` in `/Applications` on each Mac. Details in
[`desktop/README.md`](desktop/README.md).

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
duplicate renders. A crashed worker leaves its job in `running/` — `--reap`
requeues anything stuck > 45 min.

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
| `farm_worker.sh` | the claim-render-repeat loop (driven by the .command) |
| `enqueue.sh` | add jobs / seed sweeps; asks test-vs-hero at prompt time |
| `farm_status.sh` | counts, in-flight view, `--reap` for crashed jobs |
| `job.sample` | the job file format |

## How hard is this really?

Honest take: **the farm scripts are trivial; the ML toolchain install is the only
real friction** — and `setup.command` automates that.

- **Green path (all Macs identical, share reachable):** double-click
  `setup.command`, wait, double-click `start_worker.command`. Two clicks + a wait.
- **What can trip a teammate up:** approving the SMB mount in Finder the first
  time; a MacBook Pro needing a Gigabit adapter; `uv sync` needing internet
  (that's why the Macs keep WiFi); and disk space for the models. All called out
  in `setup.command`'s output as it runs.

No terminal knowledge needed beyond double-clicking `.command` files and, if they
want to queue jobs, copying an `enqueue.sh` line.

---

*MIT-licensed. LTX-2.3, MLX, and mflux are separate upstream projects under their
own licenses.*
