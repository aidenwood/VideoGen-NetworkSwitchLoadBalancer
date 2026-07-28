# OOM limits on the farm Macs

How much memory a render is allowed to use, why, and what happens when it asks
for more. Read this before changing any number in `farm.conf`.

The evidence behind these numbers is
[`MEMORY-INCIDENT-2026-07-28.md`](MEMORY-INCIDENT-2026-07-28.md) — a real OOM
that took down a machine. This file is the operating manual; that one is the lab
notebook.

**Fleet:** 3 × 32GB Macs + 1 × 64GB Mac. Nothing here assumes any particular
machine — every limit is derived from each Mac's own `hw.memsize`.

---

## ⚠️ Read this first

Two facts overturn the way this normally gets built:

**1. The old documented peaks were 3–5× too low.** `farm_worker.sh` used to
claim `full` peaks 10–16GB and `light` stays in "low single-digit GB". One
896×1216 z-image still **with `--low-ram`** measured **27.5 GB**. Every number
in the old README and profile table was fiction. They have been corrected.

**2. MLX memory is invisible to `ps` RSS.** That same 27.5 GB job reported
**4.9 GB RSS**. So the obvious design — "check free memory, then launch" — is
blind to the exact thing it is gating on. In the original incident three workers
all read "plenty free" in the same instant, all launched, and collectively
demanded ~100 GB on a 36 GB box. Jetsam killed the lot.

**Therefore: admission control, not observation.** A worker prices each job from
a static budget and a declared cost *before* it claims it. The free-memory guard
still exists, but it is a courtesy to whoever is sitting at the Mac — it is not
the OOM protection and must never be relied on as such.

---

## 1. Why Apple Silicon makes this its own problem

There is no VRAM. CPU and GPU share one pool, so a render's weights,
activations and frame buffer all come out of the same RAM macOS is using to run
everything else.

- **There is no "GPU out of memory" error to catch.** CUDA fails cleanly at the
  card's boundary. Here there is no boundary — the allocation just succeeds, and
  keeps succeeding, into swap.
- **Overshooting hurts the whole Mac.** On a daily-driver machine an unbounded
  render doesn't fail; it makes the Mac unusable first.

### What actually kills a render

macOS's memory killer, **jetsam**, SIGKILLs by footprint, biggest first — always
the render.

1. Render grows past physical RAM.
2. macOS pages to swap. Throughput collapses.
3. Swap fills, pressure hits critical.
4. Jetsam SIGKILLs the render → **rc 137** (128 + SIGKILL).

Nothing in that chain produces a useful error, and by step 4 the Mac has been
thrashing for minutes. Prevention has to happen before step 1.

## 2. The budget: 90% of each Mac's RAM

One number per machine, `MEM_BUDGET_PCT` (default 90) of installed RAM.
Deliberately **not** per-profile — `full` vs `light` selects LTX/mflux *flags*;
it doesn't change how much memory the machine physically has.

| Installed RAM | Tier | `auto` picks | Budget |
|---|---|---|---|
| 32 GB (×3) | `32GB-video-only` | `light` | **28 GB** |
| 64 GB (×1) | `64GB-everything` | `full` | **57 GB** |

`full` drops `--low-ram`. Since `--low-ram` *already* measured 27.5 GB, dropping
it on anything under 64 GB is asking for a kill — hence `AUTO_PERF_MIN_GB=64`.

## 3. Pricing a job before claiming it

```
peak_GB  ≈  megapixels × PER_MP + BASE          (video also scales by frames/97)
```

| Coefficient | Value | Basis |
|---|---|---|
| `STILL_GB_PER_MP` / `STILL_GB_BASE` | 25 / 3 | **Measured.** 896×1216 `--low-ram` = 27.5 GB. (A pure fit to that one point gives 22.5; 25 is deliberately ~10% conservative — this is a guard, it should err high.) |
| `VIDEO_GB_PER_MP` / `VIDEO_GB_BASE` | 22 / 4 | **Extrapolated. Not measured.** See §7. |

What that prices out to, against the real fleet:

| Job | Price | 32 GB Mac | 64 GB Mac |
|---|---|---|---|
| 896×1216 still (the measured one) | 30 GB | ❌ | ✅ |
| `lora_i2v` 896×1216 | 30 GB | ❌ | ✅ |
| **hero t2v 1080×1920 f97** | **49 GB** | ❌ | ✅ |
| t2v 1080×1920 f65 | 34 GB | ❌ | ✅ |
| t2v 768×1280 f97 | 25 GB | ✅ | ✅ |

### 🚩 The consequence you need to know about

**On today's coefficients, every 1080×1920 job goes to the single 64 GB Mac.**
The three 32 GB Macs can only take 768×1280-and-below work. That is a 4-Mac farm
behaving like a 1-Mac farm for hero renders.

That may well be too pessimistic — **the video coefficient is an extrapolation
from a stills measurement, not a measurement of video.** Before accepting it,
run the measurement (§7). If the real number is lower, you get your farm back.

Do **not** simply lower `VIDEO_GB_PER_MP` until jobs fit. That is how the
original incident happened.

## 4. What a worker does with a job it can't afford

It **releases it back to the queue** — not to `failed/`:

```
↩︎  released big.job — t2v 1080x1920 needs ~49GB, budget is 28GB (90% of 32GB)
```

The job is untouched and a bigger Mac picks it up. The releasing worker
remembers it in-memory so it doesn't claim-and-release in a tight loop, and
moves on to work it *can* do. Nothing is lost and no human is involved.

Jobs can also carry an explicit floor, which skips pricing entirely:

```bash
./enqueue.sh --prompt "..." --min-ram 64      # 64GB Mac only
```

## 5. When a job OOMs anyway

```
♻️  OOM (rc=137) — requeued light, now needs >33GB (attempt 1/2)
   backing off 90s to let memory drain
```

Three things happen, all deliberate:

1. **Requeued, not failed.** A jetsam kill means the price model was wrong for
   this job, not that the job is bad.
2. **`MIN_RAM_GB` is raised above this Mac's RAM.** Otherwise every 32 GB Mac in
   the farm discovers the same thing the hard way, one after another.
3. **90-second backoff.** Load-bearing. In the incident an instant retry
   reloaded a 27.5 GB model into a machine still recovering and was killed
   again — that loop is what looked like "the terminal hit 72 GB and kept
   looping". The single-box `pipeline.py` uses the same 90 s for the same reason.

After `OOM_MAX_RETRY` (default 2) it goes to `failed/` with the reason spelled
out.

## 6. Reading the fleet

```bash
./farm_status.sh
```
```
  workers:
    HOST                RAM TIER           PERF    BUDGET   FREE PRESS STATE
    studio-64           64G 64GB-everythin full       57G    62%     1 rendering
    desk-32-a           32G 32GB-video-onl light      28G    71%     1 idle
```

`PRESS` is macOS's pressure level: **1 normal, 2 warn, 4 critical**. Level 2 is
routine on a Mac someone is using. Workers refresh their row every poll and
remove it on exit, so only live machines appear.

Every successful job also records what it *actually* used:

```bash
grep -h peak_mem_gb /Volumes/RenderFarm/done/*.json | sort -u
```

That is real data accumulating on its own — use it to correct the coefficients
rather than guessing.

## 7. Open item — measure the video curve

The one number still unmeasured, and the one costing you three machines:

```bash
./measure_peak.sh              # video curve, run on the 64GB Mac
./measure_peak.sh --stills     # re-verify the still curve
./measure_peak.sh --quick      # two points instead of four
```

It renders progressively larger jobs with the farm's own instrumentation
attached, reads the true peak from each, least-squares fits the curve, and
prints the exact lines to paste into `farm.conf`. It runs uncapped on purpose —
the point is the honest peak, not one shaped by the ceiling being calibrated.

Then edit `farm.conf` on the coordinator once; every Mac re-prices its queue
within a poll.

## 8. Changing limits (one file, whole farm)

Edit **`$FARM_ROOT/farm.conf`** on the coordinator. Workers re-read it at the top
of each poll, so a change lands everywhere within `POLL_SECS` (~15 s) — no
restarts, no visiting machines.

```bash
vi /Volumes/RenderFarm/farm.conf
./farm_status.sh                     # confirm the BUDGET column moved
```

Precedence, lowest to highest:

```
farm_mem.sh defaults  <  farm.conf  <  farm.conf.<hostname>  <  env var
```

**One Mac needs to differ?** Drop a `farm.conf.<hostname>` on the share with
only the lines that change:

```bash
# /Volumes/RenderFarm/farm.conf.elijahs-macbook
: "${PERF:=light}"
```

Setting `export PERF=...` in that Mac's `start_worker.command` also works, but
it's an env var, so it beats the share — that machine then stops responding to
farm-wide changes and someone has to walk over to it. Prefer the host file.

Config lives on the share; **code** (`farm_mem.sh`, `farm_sitecustomize/`) is
read from the local checkout, so a flaky SMB mount can't take the guards down
mid-render. Shipping a code change means re-running `provision.command`.

## 9. Every knob

| Setting | Default | Meaning |
|---|---|---|
| `PERF` | `auto` | `auto` / `full` / `light`. `auto` sizes to installed RAM. |
| `AUTO_PERF_MIN_GB` | `64` | RAM needed before `auto` picks `full`. |
| `MEM_BUDGET_PCT` | `90` | % of a Mac's RAM it will admit a job against. |
| `ADMISSION` | `block` | `block` release unaffordable jobs · `warn` log and run anyway · `off` no pricing. |
| `STILL_GB_PER_MP` / `_BASE` | `25` / `3` | Still price. Measured. |
| `VIDEO_GB_PER_MP` / `_BASE` | `22` / `4` | Video price. **Extrapolated — see §7.** |
| `MLX_CAP` | `1` | Apply the budget inside the render process. |
| `OOM_BACKOFF` | `90` | Seconds to drain after a kill. Don't lower it. |
| `OOM_MAX_RETRY` | `2` | Retries before `failed/`. |
| `MEM_GUARD` | `1` | Courtesy free-RAM guard (not OOM protection). |
| `MIN_FREE_MEM_PCT` | `10` | Pause below this % free RAM. |
| `MAX_PRESSURE_LEVEL` | `4` | Pause at this pressure level. |
| `MAX_SWAP_USED_MB` | `12288` | Pause if already this deep in swap. |
| `MIN_FREE_GB` | `15` | Disk guard (not memory; here so all limits are together). |

Per-job: `PERF=`, `MIN_RAM_GB=` (`enqueue.sh --perf`, `--min-ram`).

**Every guard fails open.** If a `sysctl` can't be parsed the job proceeds — a
farm that silently stops rendering is worse than one that occasionally OOMs.

## 10. Implementation notes

- **`farm_mem.sh`** — readings, budget, pricing, guard, OOM classification.
  Side-effect free: source it and call `mem_tier`, `mem_budget_gb`,
  `est_peak_gb t2v 1080 1920 97` to check a machine by hand.
- **`farm_sitecustomize/sitecustomize.py`** — Python auto-imports a module named
  `sitecustomize` at startup, so putting this directory on `PYTHONPATH` reaches
  `uv run ltx-2-mlx` *and* `mflux-generate-*` without patching or version-pinning
  upstream. It also registers an `atexit` hook that prints the true peak, which
  is what lands in the sidecar and feeds `measure_peak.sh`. Every failure path is
  a no-op — a missing cap must never break a render.
- **The MLX cap is a mitigation, not a wall.** Tested directly: 2 GB allocated
  under a 1 GB limit, no exception. `set_memory_limit` only raises once RAM
  *and* swap are gone. It's still worth setting, because MLX's *default* limit
  is 1.5× the recommended working set — larger than physical RAM — so out of the
  box MLX will never give up before the machine swaps itself to death.
  `set_cache_limit` **is** enforced and does bound the buffer cache.
- **Detection is belt-and-braces:** exit code (137/134/139) *and* a log grep,
  because a signal doesn't always survive the
  `caffeinate → nice → uv → python` chain intact. (`pipeline.py` sees the same
  event as a *negative* returncode — Python reports signals differently.)

### bash 3.2

macOS still ships **bash 3.2** (2007) at `/bin/bash`, and these scripts run
under `set -u`. In 3.2, expanding an *empty* array with `"${arr[@]}"` is a fatal
unbound-variable error, not an empty expansion:

```bash
$ /bin/bash -c 'set -u; a=(); echo "${a[@]}"'
/bin/bash: a[@]: unbound variable
```

This was live in `farm_worker.sh`: `imgarg=()` on every `t2v` job and `largs=()`
on every non-LoRA `test` job both hit it, killing the render before it started.
Fixed by using the guarded form everywhere:

```bash
${imgarg[@]+"${imgarg[@]}"}
```

Related: under `set -e`, a short-circuited `a && b` list returns non-zero and
can abort a script from some positions. `provision.command` uses explicit `if`
blocks and `|| true` for exactly this reason.
