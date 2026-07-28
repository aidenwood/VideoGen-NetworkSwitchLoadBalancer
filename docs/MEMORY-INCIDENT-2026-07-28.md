# Memory & OOM on the farm — measured findings

> **⚠️ STATUS UPDATE — this is now IMPLEMENTED farm-wide.** Everything proposed in
> §4 and §5 below shipped: RAM detection, a 90% budget, per-job pricing with
> release-to-a-bigger-Mac, `MIN_RAM_GB`, rc=137 handling with the 90s backoff, and
> `PERF=auto` gating `full` to 64GB. It is configured from one file on the share
> (`farm.conf`) so all four Macs follow it. See **[`OOM_LIMITS.md`](OOM_LIMITS.md)**
> for the operating manual.
>
> **This file is kept as the measurement record** — the evidence behind the numbers.
> Do not change a coefficient in `farm.conf` without updating §1 here first.
>
> **§6 is still open.** The video coefficient is extrapolated, not measured, which
> currently prices hero 1080×1920 at ~49GB and reserves it for the 64GB Mac alone.
> Run `./measure_peak.sh` to close it.

**Original status:** findings + proposed design. Nothing in `farm_worker.sh` has been
changed by this document. Written 2026-07-28 from a real OOM incident on the M4 Max
(36GB) while running `Social Video Creation/_par/pipeline.py`.

**Fleet this must work on:** 3 × 32GB Macs, 1 × 64GB Mac. Workers must **detect the RAM
they actually have** and stay under **90% of it**, rather than assuming the author's box.

---

## 1. The measurement that changes things

One `mflux-generate-z-image-turbo` still, 896×1216, `-q 4`, **`--low-ram`**, Elijah LoRA,
9 steps:

```
Peak MLX memory: 27.50 GB
wall time:       ~9 min  (~59 s/step)
ps RSS:          4.9 GB      <-- see §2
```

Compare to the current header in `farm_worker.sh:20-25`:

| Profile | Header claims | Reality (measured) |
|---|---|---|
| `light` (`--low-ram`) | "low single-digit GB (well under 16)" | **27.5 GB** |
| `full` (no `--low-ram`) | "~10-16GB" | unmeasured, but **> 27.5 GB** |

Those numbers are wrong by roughly 3-5×. They are the reason a 32GB Mac will die on a job
the docs promise is safe. **Correcting `farm_worker.sh:20-25` is the single highest-value
change in this document.**

### Consequence for the fleet

| Machine | 90% budget | 896×1216 still @ 27.5GB |
|---|---|---|
| 32GB × 3 | 28.8 GB | **Does not fit.** 1.3GB nominal margin, and macOS baseline alone is 4-6GB. Guaranteed jetsam. |
| 64GB × 1 | 57.6 GB | Fits comfortably. |

So today, **`lora_i2v` and any 896×1216 still work can only run on the 64GB Mac.** The three
32GB Macs need either a lower still resolution or video-only jobs. Right now nothing in the
worker knows this, so a 32GB Mac will happily claim that job off the shared queue, OOM, and
land it in `failed/`.

---

## 2. Why the usual memory guard does not work here

**MLX unified-memory allocations are invisible to `ps` RSS, `vm_stat`, and Activity Monitor's
process list.** The 27.5GB job above reports **4.9GB RSS**.

Any gate of the form "wait until N GB free, then launch" is therefore blind to the thing it
is gating on. In the incident, three workers all read "plenty free" in the same instant, all
launched, and collectively demanded ~100GB on a 36GB box. Jetsam SIGKILLed them.

Two rules follow:

1. **Never gate MLX work on observed free memory.** Gate on a *static budget* derived from
   `hw.memsize` and a *declared cost per job type*. Admission control, not observation.
2. `MIN_FREE_GB` (`farm_worker.sh:46`) and `free_gb()` (`farm_worker.sh:202`) are about **disk**
   on the `done/` volume. They are correct as-is and are *not* a RAM guard. Don't let the
   name mislead — the farm currently has **no RAM guard at all**.

---

## 3. What the farm already gets right

`farm_worker.sh:63-79` enforces one GPU job per Mac with a PID lockfile, and the render loop
is serial. That is exactly the right call and it is already done. Nothing to fix there.

The gap is not *how many* jobs per Mac — it is *whether this Mac can afford this job at all*.

---

## 4. Proposed: RAM-aware admission control at 90%

### 4a. Detect the budget

```bash
# total physical RAM, and the 90% ceiling this worker will respect
MEM_TOTAL_GB=$(( $(sysctl -n hw.memsize) / 1073741824 ))
MEM_BUDGET_GB="${MEM_BUDGET_GB:-$(awk -v t="$MEM_TOTAL_GB" 'BEGIN{printf "%d", t*0.90}')}"
log "ram: ${MEM_TOTAL_GB}GB total, budget ${MEM_BUDGET_GB}GB (90%)"
```

Gives 28 GB on the 32GB Macs, 57 GB on the 64GB Mac, automatically. Override via env for a
machine that also runs other work.

### 4b. Declare a cost per job

Peak scales with pixel count × frames, not with the model. Only one point is measured so far,
so **treat everything except the 896×1216 row as an estimate until measured** (recipe in §6):

```bash
# echo estimated peak GB for a job. MEASURED values marked; others are extrapolated.
est_peak_gb(){
  local type="$1" w="$2" h="$3" frames="$4"
  local mp=$(( (w*h) / 1000000 ))
  case "$type" in
    lora_i2v|still)
      # MEASURED: 896x1216 (1.09MP) --low-ram = 27.5GB
      awk -v w="$w" -v h="$h" 'BEGIN{printf "%d", (w*h)/1000000 * 25 + 3}' ;;
    t2v|i2v)
      awk -v w="$w" -v h="$h" -v f="$frames" 'BEGIN{printf "%d", (w*h)/1000000 * f/97 * 22 + 4}' ;;
  esac
}
```

### 4c. Refuse rather than OOM — and route to a Mac that can

This is the important behavioural change. A job this Mac cannot afford must go **back to the
queue** so the 64GB Mac picks it up. It must not be claimed-and-failed.

```bash
need="$(est_peak_gb "$TYPE" "$WIDTH" "$HEIGHT" "$FRAMES")"
if [ "$need" -gt "$MEM_BUDGET_GB" ]; then
  log "!! $ID needs ~${need}GB > ${MEM_BUDGET_GB}GB budget on $HOST — releasing for a larger Mac"
  mv "$claimed" "$QUEUE/$(basename "${claimed%%.$HOST.*}")"
  touch "$QUEUE/.skip.$HOST.$ID"     # so this Mac doesn't instantly re-claim it
  continue
fi
```

Worth pairing with a `MIN_RAM_GB=` field in the job file so `enqueue.sh` can mark hero
1080×1920 `lora_i2v` work as 64GB-only up front, instead of every 32GB Mac discovering it
the hard way.

### 4d. Treat an OOM kill as retryable, not as a failure

A jetsam SIGKILL surfaces in bash as **exit code 137** (128 + SIGKILL). Currently
`farm_worker.sh:245` moves any non-zero rc straight to `failed/`:

```bash
mv "$claimed" "$FAILED/$(basename "$claimed").rc${rc}"
```

So an OOM looks identical to a bad prompt, and the job is dead. Suggested:

```bash
if [ "$rc" -eq 137 ] || [ "$rc" -eq 139 ]; then
  log "⚠️ $ID OOM-killed (rc=$rc) — memory model underestimated; requeueing at 64GB-only"
  echo 'MIN_RAM_GB=48' >> "$claimed"
  mv "$claimed" "$QUEUE/$(basename "${claimed%%.$HOST.*}")"
  sleep 90                      # let memory actually drain before claiming anything else
else
  mv "$claimed" "$FAILED/$(basename "$claimed").rc${rc}"
fi
```

The 90s drain matters. In the original incident the retry fired instantly, reloaded a 27.5GB
model into a machine still recovering, and was killed again — that loop is what Aiden saw as
"the terminal hit 72GB and kept looping".

Guard against a job bouncing forever: cap requeues (e.g. `REQUEUE_COUNT=` in the job file,
to `failed/` on the 3rd).

### 4e. Surface it in the Tauri app

The dashboard should show per-worker `total RAM / 90% budget / estimated peak of current job`,
and mark a worker as ineligible for oversized jobs. Right now a user has no way to see that
three of the four Macs physically cannot run the job they just enqueued.

---

## 5. Recommended profile changes

- Correct the `farm_worker.sh:20-25` header to the real numbers.
- `PERF` defaults to `full` (`farm_worker.sh:44`). On a 32GB Mac that is the wrong default —
  `full` drops `--low-ram`, and `--low-ram` was *already* at 27.5GB. **Default `PERF` by
  machine size:** `full` only when `MEM_TOTAL_GB >= 64`, else `light`.
- Add a third profile for the 32GB Macs, e.g. `tiny` = `--low-ram --tile-frames 2` plus a
  reduced still resolution, once §6 establishes what actually fits.

---

## 6. Open item — measure the resolution curve

Only one point is measured. Before trusting the estimator in §4b, run this on the 64GB Mac
for each resolution and record `Peak MLX memory` from the tail of the output:

```bash
for dim in 512x704 640x896 768x1024 896x1216; do
  w=${dim%x*}; h=${dim#*x}
  HF_HUB_OFFLINE=1 caffeinate -ims nice mflux-generate-z-image-turbo -q 4 --low-ram \
    --lora-paths "$LORA_DIR/Elijah_lora.safetensors" --lora-scales 0.9 \
    --steps 9 --guidance 1.0 --width "$w" --height "$h" --seed 9301 \
    --prompt "eljhwd man, photorealistic test" --output "/tmp/probe_$dim.png" 2>&1 \
    | tail -2 | sed "s/^/$dim /"
done
```

Fill the results into a table here, then replace the linear estimator with a lookup. The goal
is finding the largest still resolution that fits **28 GB**, which is what unlocks the three
32GB Macs for `lora_i2v` work.

`mflux` / `ltx-2-mlx` do not expose a hard MLX allocation cap on the CLI, so external
admission control (§4) is the only lever available without patching those tools.

---

## 7. One-line summary

One MLX gen job at a time is already enforced and correct — but the documented memory costs
are 3-5× too low, the worker has no RAM guard, and OOM kills are recorded as permanent
failures. Detect `hw.memsize`, budget to 90%, price each job before claiming it, bounce
oversized jobs to the 64GB Mac, and back off 90s on rc=137.

Cross-reference: `Social Video Creation/_par/pipeline.py` now implements the equivalent fixes
single-box (global `GPU` lock, `attempt()` with OOM detection via `returncode < 0` and a 90s
lock-released backoff).
