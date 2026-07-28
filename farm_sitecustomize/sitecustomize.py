"""Apply this farm's MLX memory budget inside the render process.

Python imports ``sitecustomize`` automatically at interpreter startup if it can
find one on ``sys.path``, so putting this directory on ``PYTHONPATH`` caps every
``uv run ltx-2-mlx`` and ``mflux-generate-*`` the worker launches — without
patching, forking or even pinning a version of the upstream LTX2-MLX repo.

What each setting actually does — MEASURED, not assumed:

  set_cache_limit   REAL and enforced. MLX reclaims from its free-buffer cache
                    on the next allocation once past this. Without it the cache
                    happily sits on GBs it will never reuse, which on a shared
                    daily-driver Mac is the difference between usable and not.

  set_memory_limit  A GUIDELINE, not a wall. Verified by allocating 2GB under a
                    1GB limit: it succeeded, no exception. MLX only raises once
                    the limit is passed AND physical RAM + swap are exhausted.
                    It still earns its place, because the DEFAULT limit is 1.5x
                    the recommended working set — i.e. larger than physical RAM
                    — so out of the box MLX will always thrash into swap before
                    it ever gives up. Setting it below physical RAM makes the
                    process die on its own terms instead of waiting for jetsam.

So this file is a mitigation, not the enforcement. Actual OOM prevention is the
worker's RAM guard (refuse to start when tight) and the light profile's
--low-ram + temporal tiling, which are the levers that genuinely lower peak.
See docs/OOM_LIMITS.md.

Reads (set by farm_worker.sh):
    FARM_MLX_CAP_BYTES    hard-ish ceiling for MLX allocations
    FARM_MLX_CACHE_BYTES  ceiling for MLX's free-buffer cache
    FARM_MLX_VERBOSE      set to 1 to log what was applied

Nothing here may ever break a render: MLX not being importable, an old MLX
without these setters, or a malformed value must all degrade to "no cap".
"""

import os
import sys


def _bytes_from_env(name):
    raw = os.environ.get(name, "").strip()
    if not raw:
        return None
    try:
        value = int(raw)
    except ValueError:
        return None
    return value if value > 0 else None


def _apply():
    cap = _bytes_from_env("FARM_MLX_CAP_BYTES")
    cache = _bytes_from_env("FARM_MLX_CACHE_BYTES")
    if cap is None and cache is None:
        return

    try:
        import mlx.core as mx
    except Exception:
        return  # not an MLX process (or MLX not installed) — nothing to cap

    applied = []
    # set_memory_limit / set_cache_limit moved to the top-level mlx.core
    # namespace; older builds only expose them under mlx.core.metal.
    targets = [mx]
    metal = getattr(mx, "metal", None)
    if metal is not None:
        targets.append(metal)

    for name, value in (("set_memory_limit", cap), ("set_cache_limit", cache)):
        if value is None:
            continue
        for target in targets:
            fn = getattr(target, name, None)
            if fn is None:
                continue
            try:
                fn(value)
                applied.append("%s=%.1fGB" % (name, value / 1024 ** 3))
            except Exception:
                continue
            break

    if applied and os.environ.get("FARM_MLX_VERBOSE") == "1":
        print("[farm] MLX budget: " + ", ".join(applied), file=sys.stderr)

    _install_peak_reporter(mx)


def _install_peak_reporter(mx):
    """Print the render's real peak memory on exit.

    The worker scrapes this into the job's .json sidecar, which turns the
    budgets in farm.conf from guesses into something you can retune against
    actual measurements. Best-effort only.
    """
    get_peak = getattr(mx, "get_peak_memory", None)
    if get_peak is None:
        metal = getattr(mx, "metal", None)
        get_peak = getattr(metal, "get_peak_memory", None) if metal else None
    if get_peak is None:
        return

    import atexit

    def _report():
        try:
            print(
                "[farm] MLX peak: %.2f GB" % (get_peak() / 1024 ** 3),
                file=sys.stderr,
            )
        except Exception:
            pass

    atexit.register(_report)


try:
    _apply()
except Exception:
    pass  # a memory cap is an optimisation; never let it stop a render
