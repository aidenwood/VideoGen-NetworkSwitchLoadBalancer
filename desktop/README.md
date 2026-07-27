# FarmMon — render-farm menubar app

A native macOS menubar app (Tauri v2) that watches the shared farm folder and gives
you **live tray status + notification sounds** when pings move through the farm — so
everyone on the team sees, and hears, when a job is dispatched, when a Mac picks it
up, and when a render finishes.

## What it does

- **Menubar tray** with a live tooltip: `queued N · running N · done N · failed N`.
- **Dashboard window** (tray → *Open dashboard*): counts + a live feed of recent pings.
- **Native notification + distinct sound** on each event:

| Event | Meaning | Notification | Sound |
|---|---|---|---|
| new `queue/*.job` | a job was **dispatched** | 📤 Ping sent | Tink |
| new `running/*` | a Mac **picked it up** | 📥 Ping received (`host`) | Ping |
| new `done/*.ok` | a Mac **finished** a render | ✅ Render done (`host`) | Glass |
| new `failed/*` | a render **failed** | ❌ Render failed | Basso |

It **polls the share every 2s** rather than using file-system events, because macOS
FSEvents don't fire reliably for changes another Mac makes over an SMB mount. Polling
is bulletproof across the network.

## Run / build

```bash
cd desktop
npm install
npm run tauri dev      # run live
npm run tauri build    # -> src-tauri/target/release/bundle/macos/FarmMon.app (+ .dmg)
```

Point it at your share with `FARM_ROOT` (defaults to `/Volumes/RenderFarm`):

```bash
FARM_ROOT=/Volumes/RenderFarm open -a FarmMon
```

Ship the built `FarmMon.app` to each teammate's `/Applications` and it runs in their
menubar. First launch may ask to allow Notifications — say yes.

## Layout

```
desktop/
  package.json            # @tauri-apps/cli
  ui/index.html           # dashboard (dark, polls get_state every 2s)
  src-tauri/
    Cargo.toml
    tauri.conf.json       # menubar app: Accessory activation, withGlobalTauri
    capabilities/default.json
    icons/                # app icon + template tray glyph (generated)
    src/lib.rs            # tray, poll-watcher, notifications, sounds, get_state
    src/main.rs
```

> Unsigned builds: right-click → Open the first time (or sign with your Apple
> Developer ID for `codesign`/notarisation before wider distribution).
