# LTX Mac Farm — render-farm app (menubar + web gateway)

A native macOS menubar app (Tauri v2) that watches the shared farm folder — and serves
**the same interface in a browser** so the whole team can use the farm from their own
desk, another Mac, or a phone on the office network.

Five views, one app:

| View | What it's for |
|---|---|
| **Setup** | guided, role-aware setup for THIS Mac — every README stage as a button |
| **Farm** | the glance: counts, queue lane, activity, runs, and this farm's own numbers |
| **Board** | the pipeline as a kanban board — plan, queue, reorder, filter, requeue, variants |
| **Review** | the cherry-pick loop: proof stills and finished clips, approve or send back |
| **Team** | who's connected, whose Mac is rendering what, and how each one is doing |
| **Checks** | live ✅/⚠️/❌ per setup step, farm-wide limits, operations, autopilot, gateway |

## The web gateway

The menubar popover only helps the person sitting at that Mac. The gateway serves the
**same `ui/index.html`** over HTTP, so:

- the producer watches the board from their own desk;
- whoever set up Mac 3 can finish it from Mac 1 (Team → *Open their board*);
- anyone can **watch or download a finished clip in the browser** — no SMB mount;
- a phone can queue a clip on the way to a shoot.

```
tray → Open in browser      http://127.0.0.1:8787/?k=<key>     (this Mac)
tray → Copy team link       http://<host>.local:8787/?k=<key>  (LAN sharing on)
```

It opens automatically when the app starts (Settings → *Web gateway* → uncheck to stop).

**Security, plainly.** The gateway can queue renders and run setup steps, so:

- it binds to **127.0.0.1 by default** — nothing off this Mac can reach it;
- LAN access is **opt-in per Mac**, with a plain-English warning next to the checkbox;
- once on the LAN, every request needs a **random 32-hex key** (it's in the team link);
- `/file` downloads are confined to the farm folder **and** to media extensions, so
  `?path=` can't be used to read the rest of the disk;
- it's HTTP with the key in the URL: right for a private office switch, **not** something
  to port-forward to the internet.

### Headless Macs

```bash
"/Applications/LTX Mac Farm.app/Contents/MacOS/ltx-mac-farm" --serve
```

Gateway only — no menubar, no window. A render node in a cupboard still publishes its
presence to the Team view and can still be set up from a browser.

## Notifications

A "ping" is a job moving through the pipeline. Each one fires a native notification and
a distinct sound, so the office hears the farm working:

| Event | Meaning | Notification | Sound |
|---|---|---|---|
| new `queue/*.job` | a job was **dispatched** | 📤 Ping sent | Tink |
| new `running/*` | a Mac **picked it up** | 📥 Ping received (`host`) | Ping |
| new `done/*.ok` | a Mac **finished** a render | ✅ Render done (`host`) | Glass |
| new `failed/*` | a render **failed** | ❌ Render failed | Basso |

It **polls the share every 2s** rather than using file-system events, because macOS
FSEvents don't fire reliably for changes another Mac makes over an SMB mount.

## How the board actually reorders work

Workers claim jobs with `for cand in queue/hi/*.job queue/*.job` — a bash glob, so **claim
order is byte order on the filename**, and the stamp is the first thing in it. So the board
doesn't keep a private ordering table; dragging a card **renames the file**:

- **drag to reorder** → renamed `00000000_000000_<seq>__<id>.job`, a date that can't occur,
  so hand-ordered work stays ahead of anything enqueued later;
- **↑ Priority** → moved into `queue/hi/`, which workers scan first;
- **Requeue** → the worker's `.job.<HOST>.<pid>.rcN` suffix is stripped and it goes back
  to `queue/` with a fresh stamp;
- **Run again** on a finished job **copies** it, so the original render and its mp4 survive.

A claimed job can't be cancelled or reordered from the board — that file belongs to a
worker mid-render, and yanking it would leave half an mp4 and a confused Mac.

The share is the only database. Nothing here caches state, because five Macs write to that
folder at once and a second source of truth would immediately disagree with it.

## Working at sweep scale

A 200-clip sweep makes plain lanes unreadable, so the board has:

- **search** across prompts, names, people and runs, plus filters for size, Mac,
  run and review state — applied in the browser on data already fetched;
- **multi-select** with a bulk bar that only offers what's legal for the whole
  selection (a mixed-lane selection says so instead of half-working);
- **keyboard**: `1`–`6` switch views, `/` search, `a` select the queued lane,
  `p` promote, `x` remove, `r` requeue, `Esc` close the drawer;
- **estimates from this farm's own history**: every card says roughly how long it
  takes and, in claim order, when it should start. The simulation frees a slot
  when a running job's estimate runs out, so the fourth job on one Mac is shown
  as four renders away rather than sharing one fake ETA.

## Review — the cherry-pick loop

`test`-mode jobs render a proof still in seconds. The Review tab lays them out as
a contact sheet (whole frame visible, not cropped — you're judging framing), and
each fresh still offers **Render hero** to spend real farm time on the winner. A
still whose hero render already exists says so, so nobody renders it twice.

Finished clips get the same treatment with poster frames: **Approve** or **Needs
another take**, stored as `reviews/<ID>.json` on the share. Autopilot reads that
too — a clip a human marked *retake* is never auto-retried.

Poster frames are one `ffmpeg` frame, cached in `done/.thumbs/`, generated on
first request and reused by every teammate's browser.

## Overnight runs and autopilot

**Plan a run** takes a pasted shot list — one prompt per line — × delivery sizes ×
takes, tags every job with one run name, and tells you the size of the night
before you commit it. Jobs are stamped prompt-major, so an interrupted night still
covers every prompt rather than 40 versions of the first one.

**Autopilot** (off by default, one Mac only) is what makes the night unattended:

| It does | Because |
|---|---|
| requeues a job whose worker went quiet | a Mac that dies at 2am shouldn't cost you the job |
| retries a failure (configurable, default once) | one flake shouldn't end a shot |
| on a memory kill, asks for a bigger Mac — or shrinks the job | retrying an OOM at the same size just dies again |
| **pauses the whole queue** after N failures in a row | a broken setup would otherwise burn the remaining six hours |
| writes every action to `logs/autopilot.log` | an unattended system that can't explain itself isn't trustworthy |

It only ever *requeues* work — it never deletes a job and never touches a file a
worker holds. Exactly one Mac acts, decided by a heartbeat lock at
`runs/.autopilot.lock`; two babysitters would double the night's work.

When a run finishes, whoever holds the shift gets a notification with the tally,
and the **Report** button on the Farm view opens the morning digest: what landed,
what failed (as actionable cards), who rendered what, and how much was approved.

## Operations without Terminal

- **Reap** — requeue jobs whose worker died (`farm_status.sh --reap`, as a button).
- **Pause / Resume** — moves waiting jobs into `queue/hold/`, which the workers'
  glob can't see. In-flight renders finish normally; the priority lane survives.
- **Farm-wide limits** — edits `farm.conf` *on the share*, so every worker picks
  the change up within one poll. That file is `source`d by bash on every Mac, so
  each key is whitelisted and every value validated: numbers are range-checked,
  choices are dropdowns, and `MODEL` is restricted to a repo-shaped string.
- **This farm's numbers** — clips/hour per Mac, average by delivery size, and how
  many renders peaked above their memory budget (the number that retunes
  `MEM_BUDGET_PCT` from evidence instead of guesswork). All from the sidecars the
  worker already writes.

## Image-to-video, LoRAs and presets

The composer builds all three job types. For `i2v` it lists what's in `assets/`
and lets you **drop an image straight onto the page** (uploaded via the gateway,
48MB cap, images only, filename sanitised). For `lora_i2v` it lists the
`.safetensors` on the share and asks for the still prompt. A setup can be saved as
a **preset** — shape only, never the prompt, because a preset is a format not a shot.

## Variants

Every card offers *"same shot, but…"* — generated in `jobs.rs`, so the popover and the
browser offer the same set:

- **other delivery sizes** (9:16, 1:1, 16:9, 4:5) at the same seed, pre-ticked, because
  "we need a square one" is the request that actually arrives;
- **prompt edits** that change the look, not the subject (golden hour, storm mood, slow
  push in, wide establishing, handheld) — seed kept so they stay comparable;
- **a 4-seed sweep** the farm splits across Macs;
- **a cheap proof still** — seconds instead of an hour, to check framing first.

Tick what you want, one click, N jobs queued. Frame counts are snapped to the model's
`8k+1` rule, and prompts are escape-hardened before being written into a file a worker
will `source`.

## Team presence

Each running copy of the app writes `presence/<host>.json` on the share every ~10s. The
Team view merges three sources per Mac, any of which can be missing:

| Source | Gives |
|---|---|
| `presence/<host>.json` (this app) | person's name, Mac model, RAM, role, their gateway link |
| `running/.worker.<HOST>.info` (farm_worker.sh) | profile, memory budget, free %, swap, state |
| `running/*.heartbeat` | what it's rendering **right now**, and for how long |

So a producer running only the app appears (flagged *no worker running*), and a headless
worker with no app appears too (flagged *app not running*).

## Phone

The gateway serves a web manifest and icon, so the board can be added to a home
screen and run standalone. Tap the bell in the header to get a notification as
each render lands — off until asked, remembered per browser.

## Architecture — one command surface

```
ui-react (React 19 + TS + Vite)
   │
   └── api.ts call(cmd, args)
         ├── window.__TAURI__ present?  → invoke("bridge", {cmd, args})
         └── otherwise (browser)        → POST /api/invoke {cmd, args}
                                                │
                                  Core::dispatch(cmd, args)   ← the only door
```

One frontend, one dispatch table. A feature can't exist in the popover and be missing in
the browser — they are the same bundle.

**The dead-button check now spans the language boundary.** The UI declares every command it
may call in `ui-react/src/commands.ts`; `call()` only accepts one of those, so a typo is a
compile error. A cargo test and `--selftest` then prove that list and Rust's `COMMANDS` are
the same set, and that the built bundle actually contains each name (a stale `dist/` fails
too). Four shipped setup bugs earned that check.

**How the frontend reaches both surfaces.** `src-tauri/build.rs` walks `ui-react/dist` at
compile time and generates an asset table of `include_bytes!` entries, so the bundle lives
*inside* the binary: Tauri loads it, and the gateway serves the same bytes. Vite
content-hashes its filenames, which is why the table is generated rather than a list of
`include_str!` calls. Build without a frontend and the gateway serves a page that says
`npm run build` instead of a blank window.

Both surfaces poll `get_state` every 2s, which carries a `rev` counter. Change a setting in
the popover and an open browser tab reloads its config within one poll.

## Run / build / test

```bash
cd desktop
npm install
npm run dev            # tauri dev
npm run build          # -> src-tauri/target/release/bundle/macos/"LTX Mac Farm.app" (+ .dmg)

npm run ui             # frontend only, on Vite's dev server
npm run ui:build       # build the frontend (tsc --noEmit && vite build)

npm test               # build the UI, run the behaviour suite (223 checks) + cargo (59)
npm run test:ui        # build + UI suite only
npm run test:shots     # + screenshots into test/shots/
npm run selftest       # drive every wizard path + the gateway headlessly
```

`npm run dev` starts Vite and Tauri together (hot reload). The behaviour suite serves the
**built** bundle over HTTP and stubs the backend, so it tests what ships — which is why
every test script builds the UI first.

Config lives at `~/Library/Application Support/design.aidxn.ltx-mac-farm/config.json`.
Set `FARM_CONFIG_DIR` to point at a throwaway one (that's how the tests stay off your real
install, and how you can run a second, demo setup on one Mac).

## Layout

```
desktop/
  test/ui.test.js         # behaviour tests: stubs the backend, serves the real bundle
  src-tauri/
    tauri.conf.json       # menubar app: Accessory activation, withGlobalTauri
    src/lib.rs            # config, watcher, setup checks, presence, Core::dispatch, tray
    src/jobs.rs           # the pipeline as data: board, reorder, enqueue, variants,
                          #   members, reviews, proofs, stats, farm.conf, runs, autopilot
    src/web.rs            # the HTTP gateway: auth, /api/invoke, /file with Range support
    build.rs              # embeds ui-react/dist as an asset table
    src/main.rs           # --selftest | --serve | menubar app
  ui-react/
    src/api.ts            # call() — the one funnel, typed to commands.ts
    src/commands.ts       # every command the UI may call (checked against Rust)
    src/types.ts          # the shapes dispatch returns (mirrors the serde structs)
    src/styles.css        # the tuned stylesheet, unchanged by the React port
    src/App.tsx           # shell: header, tabs, the 2s heartbeat, tray bridge
    src/views/            # Farm · Board · Review · Team · Checks · Setup
```

> Unsigned builds: right-click → Open the first time (or sign with your Apple
> Developer ID for `codesign`/notarisation before wider distribution).
