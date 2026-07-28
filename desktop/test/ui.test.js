/* ===========================================================================
   UI test harness for the menubar popover.
   ---------------------------------------------------------------------------
   The popover can't be driven from a terminal, which is how four setup bugs
   reached the user before anything caught them. `--selftest` covers the Rust
   side; this covers the UI, by stubbing window.__TAURI__ and loading the REAL
   index.html — no mock copy to drift out of sync.

   It asserts behaviour, not just that pixels rendered: buttons disable while
   working, failures surface where the user is looking, the wizard advances,
   the lane maths adds up. Console errors fail the run.

     node test/ui.test.js            # run everything
     node test/ui.test.js --shots    # also write screenshots to test/shots/
   =========================================================================== */
"use strict";
const { chromium } = require("playwright");
const path = require("path");
const fs = require("fs");
const http = require("http");

const DIST = path.resolve(__dirname, "../ui-react/dist");

/* EVERY page is served over HTTP now, not file://.
   The UI is a Vite bundle — an ES module — and Chromium refuses to load modules
   from file:// (CORS). This server stands in for the Rust gateway: it hands over
   the built bundle and fakes the two media routes. The page's own /api/invoke
   calls are stubbed inside the page, so this server never sees them. */
const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".json": "application/json",
};
const PIXEL = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg==",
  "base64");

let webBase = "";
function startStaticServer(){
  if(!fs.existsSync(path.join(DIST, "index.html"))){
    console.error("!! no built UI at ui-react/dist — run: npm run ui:build");
    process.exit(1);
  }
  return new Promise((resolve) => {
    const srv = http.createServer((req, res) => {
      const url = (req.url || "/").split("?")[0];
      // Stand in for the gateway's media routes with a 1×1 PNG, so poster frames
      // and stills behave like they do in the real thing.
      if(url.startsWith("/poster") || url.startsWith("/file")){
        res.writeHead(200, { "Content-Type": "image/png" });
        res.end(PIXEL);
        return;
      }
      const file = path.join(DIST, url === "/" ? "/index.html" : url);
      if(file.startsWith(DIST) && fs.existsSync(file) && fs.statSync(file).isFile()){
        res.writeHead(200, { "Content-Type": MIME[path.extname(file)] || "application/octet-stream" });
        res.end(fs.readFileSync(file));
        return;
      }
      res.writeHead(404).end("no");   // /manifest.json etc: not here
    });
    srv.listen(0, "127.0.0.1", () => {
      webBase = `http://127.0.0.1:${srv.address().port}/index.html?k=testkey`;
      resolve(srv);
    });
  });
}
const SHOT_DIR = path.resolve(__dirname, "shots");
const WANT_SHOTS = process.argv.includes("--shots");

let pass = 0, fail = 0;
const failures = [];
function ok(name)      { pass++; console.log(`  \x1b[32m✓\x1b[0m ${name}`); }
function bad(name, why){ fail++; failures.push(`${name} — ${why}`); console.log(`  \x1b[31m✗\x1b[0m ${name}\n      ${why}`); }
function check(cond, name, why){ cond ? ok(name) : bad(name, why || "assertion failed"); }
function group(t){ console.log(`\n\x1b[1m── ${t} ──\x1b[0m`); }

/* --- the fake backend -----------------------------------------------------
   Mirrors the real command surface. `calls` records everything the UI invokes
   so tests can assert on dispatch, and `fails` forces a command to reject.

   The UI funnels every call through the single `bridge` command (so the popover
   and the web gateway share one dispatch table in Rust). The stub unwraps that,
   which keeps every assertion written in terms of real command names — and
   means a regression in the funnel itself shows up here. */
function backend(scene, patch = ''){
  return `
    window.__calls = [];
    window.__fails = {};
    const SCENE = ${JSON.stringify(scene)};
    ${patch}                       /* per-test scene tweaks, e.g. no assets */
    const EMPTY_BOARD = { board:{ root:"/Volumes/RenderFarm", reachable:true,
      queued:[], running:[], done:[], failed:[], totals:{} } };
    window.__dispatch = async (cmd, args) => {
      args = args || {};
      window.__calls.push({ cmd, args });
      if (window.__fails[cmd]) throw new Error(window.__fails[cmd]);
      switch(cmd){
        case "setup_steps": return SCENE.setup;
        case "get_state":   return Object.assign({ rev: window.__rev || 1 }, SCENE.state);
        case "verify_link": return SCENE.verify;
        case "get_config":  return SCENE.config;
        case "discover_coordinators": return SCENE.hosts || [];
        case "run_action":  return "ran " + args.action;
        case "set_role":       SCENE.setup.role = args.role; return null;
        case "set_coordinator":return null;
        case "finish_wizard":  return null;
        case "save_config":    return { config:{}, gateway: SCENE.gateway || null };
        case "pick_repo":      return "Farm scripts: /somewhere";
        case "get_board":      return SCENE.boardData || EMPTY_BOARD;
        case "get_members":    return SCENE.team || { you:"AIDENWOOD", member:"Aiden", reachable:true, members:[] };
        case "enqueue_job":    return { files:["x.job"], message:"Queued." };
        case "job_action":     return { message:"did " + args.action };
        case "job_variants":   return SCENE.variants || { job:{}, variants:[] };
        case "set_member":     return { member: args.name };
        case "list_assets":    return SCENE.assets || { images:[], loras:[] };
        case "get_proofs":     return SCENE.review || { proofs:[], clips:[], reachable:true };
        case "set_review":     return { message:"marked " + args.id };
        case "get_stats":      return SCENE.stats || { stats:{ clips:0 }, members:[], reachable:true };
        case "get_job_log":    return SCENE.log || { lines:["loading model","step 12/40"], step:12, total:40, percent:30, path:"/x.log" };
        case "get_farm_conf":  return SCENE.conf || { path:"/Volumes/RenderFarm/farm.conf", exists:true, keys:[] };
        case "save_farm_conf": return { message:"updated " + Object.keys(args.keys || {}).length + " setting(s)" };
        case "farm_action":    return { message:"did " + args.action };
        case "plan_run":       return { run:"overnight", queued:12, message:"Queued 12 job(s) as “overnight”." };
        case "get_runs":       return { runs: (SCENE.boardData && SCENE.boardData.runs) || [] };
        case "get_run_report": return SCENE.report || { run:args.run, counts:{done:0,failed:0,queued:0,running:0,approved:0,retake:0}, done:[], failed:[], per_host:[], render_secs:0, finished:true };
        case "get_autopilot":  return SCENE.autopilot || { on:false, you:"AIDENWOOD", supervisor:"", policy:{retry:1,stale_min:20,fail_streak:5}, held:0, log:[] };
        case "set_autopilot":  Object.assign((SCENE.autopilot = SCENE.autopilot || { policy:{} }), { on: args.on }); return SCENE.autopilot;
        case "save_preset":    return { message:"Saved", presets:[{ name:args.name, job:args.job }] };
        case "delete_preset":  return { message:"Deleted", presets:[] };
        default: return null;
      }
    };
    window.__TAURI__ = { core: { invoke: async (cmd, args) => {
      if(cmd === "bridge") return window.__dispatch(args.cmd, args.args);
      return window.__dispatch(cmd, args);      // legacy shape, still honoured
    }}};
  `;
}

/* The browser surface: no Tauri at all, a stubbed fetch standing in for the
   gateway. This is how we prove the same file works on someone else's desk. */
function webBackend(scene, patch = ''){
  return backend(scene, patch) + `
    delete window.__TAURI__;
    window.fetch = async (url, opts) => {
      if(String(url).indexOf("/api/invoke") === 0 || String(url).indexOf("/api/invoke") > -1){
        const body = JSON.parse(opts.body);
        try {
          const data = await window.__dispatch(body.cmd, body.args);
          return { ok:true, status:200, json: async () => ({ ok:true, data }) };
        } catch(e) {
          return { ok:true, status:200, json: async () => ({ ok:false, error:String(e.message||e) }) };
        }
      }
      return { ok:true, status:200, text: async () => "log line 1\\nlog line 2" };
    };
  `;
}

const step = (o) => Object.assign(
  { id:"x", title:"Step", body:"Body text.", done:false, detail:"detail",
    action:"", action_label:"", manual:false }, o);

// A board card, with the same defaults jobs.rs fills in.
const card = (o) => Object.assign(
  { file:"stamp__id.job", path:"/Volumes/RenderFarm/queue/stamp__id.job", lane:"queued",
    id:"id", stamp:"stamp", priority:"normal", position:1, prompt:"a prompt", kind:"t2v",
    mode:"hero", width:1080, height:1920, frames:97, seed:42, fps:24, perf:"", lora:"",
    image:"", min_ram_gb:0, oom_retry:0, host:"", age_secs:5, mp4:"", mp4_mb:0, proof:"",
    log:"", rc:"", duration_secs:0, peak_mem_gb:0, aspect:"9:16",
    member:"", run:"", retry:0, review:"", review_by:"", review_note:"",
    est_secs:1800, eta_secs:0 }, o);

const SCENES = {
  fresh: {
    setup: { host:"desk-32-a", role:"", root:"/Volumes/RenderFarm", steps:[], all_done:false, wizard_done:false },
    state: { root:"", counts:{queued:0,running:0,done:0,failed:0}, events:[] },
    verify:{ host:"desk-32-a", root:"/Volumes/RenderFarm", is_coordinator:false, ok:0,warn:0,fail:3, checks:[], workers:[] },
    config:{ config:{}, resolved:{}, config_file:"~/…/config.json" },
  },
  worker: {
    setup: { host:"desk-32-a", role:"worker", root:"/Volumes/RenderFarm", all_done:false, wizard_done:false,
      steps:[
        step({id:"pick",  title:"Choose the coordinator Mac", detail:"Nothing chosen yet"}),
        step({id:"mount", title:"Connect to the shared folder", detail:"Not mounted", action:"mount_share", action_label:"Connect"}),
        step({id:"toolchain", title:"Install the render toolchain", detail:"Not installed", action:"run_setup", action_label:"Run setup"}),
        step({id:"models", title:"Copy the models to this Mac", detail:"No models yet", action:"run_provision", action_label:"Provision"}),
      ]},
    state: { root:"/Volumes/RenderFarm", counts:{queued:0,running:0,done:0,failed:0}, events:[] },
    verify:{ host:"desk-32-a", root:"/Volumes/RenderFarm", is_coordinator:false, ok:1,warn:0,fail:2, checks:[], workers:[] },
    config:{ config:{}, resolved:{}, config_file:"~/…/config.json" },
    hosts: ["mac-studio","elijah-mbp"],
  },
  coord: {
    setup: { host:"AIDENWOOD", role:"coordinator", root:"/Users/aidenwood/RenderFarm", all_done:false, wizard_done:false,
      steps:[
        step({id:"folder",  title:"Create the shared folder", done:true, detail:"/Users/aidenwood/RenderFarm exists"}),
        step({id:"sharing", title:"Turn on File Sharing", done:true, detail:"Sharing over SMB"}),
        step({id:"dirs",    title:"Create the queue folders", done:true, detail:"queue ready"}),
        step({id:"toolchain", title:"Install the render toolchain", detail:"Not installed on this Mac yet",
              body:"Homebrew, uv, LTX2-MLX and mflux. 15–30 min, mostly unattended.",
              action:"run_setup", action_label:"Run setup"}),
        step({id:"stage",   title:"Put the models on the share", detail:"Ready to stage",
              action:"seed_assets", action_label:"Stage models on the share"}),
      ]},
    state: { root:"/Users/aidenwood/RenderFarm", counts:{queued:0,running:0,done:0,failed:0}, events:[] },
    verify:{ host:"AIDENWOOD", root:"/Users/aidenwood/RenderFarm", is_coordinator:true, ok:8,warn:1,fail:0, checks:[], workers:[] },
    config:{ config:{coordinator:"AIDENWOOD",share_name:"RenderFarm",perf:"auto",min_free_gb:15}, resolved:{}, config_file:"~/…/config.json" },
  },
  busy: {
    setup: { host:"AIDENWOOD", role:"coordinator", root:"/Users/aidenwood/RenderFarm",
             all_done:true, wizard_done:true, steps:[step({done:true})] },
    state: { root:"/Users/aidenwood/RenderFarm",
             counts:{queued:12,running:3,done:47,failed:2},
             workers:[
               {host:"AIDENWOOD",state:"rendering",job:"hero_roof_s1003",age_secs:12},
               {host:"desk-32-a",state:"rendering",job:"hero_roof_s1005",age_secs:40},
               {host:"desk-32-b",state:"idle",job:"",age_secs:5},
             ],
             events:[
               {kind:"sent",id:"hero_roof_s1004",host:"?"},
               {kind:"received",id:"hero_roof_s1003",host:"desk-32-a"},
               {kind:"done",id:"hero_roof_s1002",host:"mac-studio"},
               {kind:"failed",id:"hero_roof_s1001",host:"desk-32-b"},
             ]},
    verify:{ host:"AIDENWOOD", root:"/Users/aidenwood/RenderFarm", is_coordinator:true, ok:9,warn:0,fail:0,
      checks:[
        {id:"a",stage_label:"This Mac",label:"Farm folder reachable",status:"ok",detail:"/Users/aidenwood/RenderFarm",fix:"",action:"",action_label:""},
        {id:"b",stage_label:"This Mac",label:"Toolchain built",status:"ok",detail:"ltx-2-mlx present",fix:"",action:"",action_label:""},
        {id:"c",stage_label:"Network",label:"Wi-Fi above Ethernet",status:"warn",detail:"Ethernet is first",
         fix:"System Settings → Network → ⋯ → Set Service Order",action:"open_network",action_label:"Open Network settings"},
      ],
      workers:[
        {host:"AIDENWOOD",state:"rendering",job:"hero_roof_s1003",age_secs:12},
        {host:"desk-32-a",state:"rendering",job:"hero_roof_s1005",age_secs:40},
        {host:"desk-32-b",state:"idle",job:"",age_secs:5},
      ]},
    config:{ config:{coordinator:"AIDENWOOD",share_name:"RenderFarm",perf:"auto",min_free_gb:15,web_open_on_launch:true},
             resolved:{root:"/Users/aidenwood/RenderFarm"}, config_file:"~/…/config.json",
             gateway:{ running:true, enabled:true, port:8787, lan:true,
                       local_url:"http://127.0.0.1:8787/?k=abc", lan_url:"http://aidenwood.local:8787/?k=abc", token:"abc" } },
    gateway:{ running:true, enabled:true, port:8787, lan:true,
              local_url:"http://127.0.0.1:8787/?k=abc", lan_url:"http://aidenwood.local:8787/?k=abc", token:"abc" },
    assets: { images:["elijah.png","roof.jpg"], loras:["Elijah_lora.safetensors"] },
    review: {
      reachable: true,
      proofs: [
        { id:"proof_a", path:"/Users/aidenwood/RenderFarm/done/proofs/proof_a_seed1000.png", seed:1000,
          age_secs:120, review:"", prompt:"assessor on a roof", width:1080, height:1920,
          done_file:"s7__proof_a.job.DESK-A.9.ok", rendered:false },
        { id:"proof_b", path:"/Users/aidenwood/RenderFarm/done/proofs/proof_b_seed1001.png", seed:1001,
          age_secs:300, review:"approved", prompt:"hail on tiles", width:1080, height:1920,
          done_file:"s6__proof_b.job.DESK-A.9.ok", rendered:true },
      ],
      clips: [
        card({ lane:"done", id:"hero_roof_fin", host:"DESK-B", mp4:"/Users/aidenwood/RenderFarm/done/hero_roof_fin.mp4",
               mp4_mb:24, duration_secs:1830, prompt:"finished hero clip", review:"" }),
      ],
    },
    stats: { reachable:true, members:[], stats: {
      clips:42, clips_24h:12, secs_24h:21600, avg_secs:1750, over_budget:2, sample:42,
      per_host:[{host:"AIDENWOOD",clips:20,secs:35000,avg_secs:1750,clips_24h:7,peak_mem_gb:44,budget_gb:57.6},
                {host:"DESK-32-A",clips:22,secs:38500,avg_secs:1750,clips_24h:5,peak_mem_gb:21,budget_gb:21.6}],
      by_size:[{label:"1080×1920 · 97f · hero",width:1080,height:1920,frames:97,mode:"hero",clips:30,avg_secs:1800},
               {label:"1920×1080 · 97f · hero",width:1920,height:1080,frames:97,mode:"hero",clips:12,avg_secs:1650}],
    }},
    conf: { path:"/Users/aidenwood/RenderFarm/farm.conf", exists:true, keys:[
      { key:"MEM_BUDGET_PCT", label:"Memory budget %", help:"share of RAM a render may use", kind:"int", choices:[], min:40, max:95, value:"90" },
      { key:"ADMISSION", label:"Admission control", help:"block = leave a job it can't afford", kind:"choice", choices:["block","warn"], min:0, max:0, value:"block" },
      { key:"MODEL", label:"Model", help:"HuggingFace repo", kind:"text", choices:[], min:0, max:0, value:"dgrauet/ltx-2.3-mlx-q4" },
    ]},
    autopilot: { on:true, you:"AIDENWOOD", supervisor:"AIDENWOOD",
      policy:{ retry:1, stale_min:20, fail_streak:5 }, held:3,
      log:["2026-07-29 03:12:04  [AIDENWOOD] requeued 2 stalled","2026-07-29 04:01:00  [AIDENWOOD] retried 1"] },
    report: { run:"overnight", counts:{ done:12, failed:2, queued:0, running:0, approved:3, retake:1 },
      render_secs:21600, finished:true, per_host:[{host:"AIDENWOOD",clips:7},{host:"DESK-32-A",clips:5}],
      done:[card({ lane:"done", id:"night_01", host:"AIDENWOOD", mp4:"/x/night_01.mp4", duration_secs:1800, run:"overnight" })],
      failed:[card({ lane:"failed", id:"night_09", host:"DESK-32-B", rc:"137", run:"overnight" })] },
    boardData: { held: 3,
      runs: [
        { run:"overnight", note:"", by:"Aiden", created_ts:0, planned:14, proof_first:false,
          queued:0, running:0, done:12, failed:2, render_secs:21600, finished:true },
        { run:"morning_fixes", note:"", by:"Elijah", created_ts:0, planned:4, proof_first:true,
          queued:2, running:1, done:1, failed:0, render_secs:600, finished:false },
      ],
      board: {
      root:"/Users/aidenwood/RenderFarm", reachable:true,
      totals:{queued:3,running:1,done:1,failed:1},
      queued:[
        card({ file:"s1__a.job", id:"hero_roof_a", position:1, priority:"high", prompt:"storm clouds over a QLD tile roof",
               member:"Aiden", run:"overnight", est_secs:1800, eta_secs:0 }),
        card({ file:"s2__b.job", id:"hero_roof_b", position:2, prompt:"hail on a driveway, slow motion",
               member:"Elijah", run:"overnight", width:1920, height:1080, aspect:"16:9", est_secs:1650, eta_secs:1800 }),
        card({ file:"s3__c.job", id:"proof_c", position:3, mode:"test", prompt:"assessor on a roof",
               member:"Aiden", est_secs:90, eta_secs:3450 }),
      ],
      running:[ card({ file:"s0__live.job.DESK-A.9", lane:"running", id:"hero_roof_live", host:"DESK-A", age_secs:412,
                       prompt:"drone pull-back over a storm-damaged street" }) ],
      done:[ card({ file:"s9__fin.job.DESK-B.9.ok", lane:"done", id:"hero_roof_fin", host:"DESK-B",
                    mp4:"/Users/aidenwood/RenderFarm/done/hero_roof_fin.mp4", mp4_mb:24, duration_secs:1830,
                    peak_mem_gb:41.5, prompt:"finished hero clip", member:"Aiden", run:"overnight" }) ],
      failed:[ card({ file:"s8__bad.job.DESK-C.9.rc137", lane:"failed", id:"hero_roof_bad", host:"DESK-C", rc:"137",
                      log:"/Users/aidenwood/RenderFarm/logs/hero_roof_bad.DESK-C.log", prompt:"the one that OOMed",
                      member:"Elijah", run:"overnight" }) ],
    }},
    variants: { job:{ id:"hero_roof_a" }, variants:[
      { group:"size", label:"Square 1:1", why:"Feed posts, LinkedIn", job:{ id:"a_1080x1080", prompt:"storm clouds", width:1080, height:1080, frames:97, seed:42, mode:"hero", sweep:0 } },
      { group:"size", label:"Landscape 16:9", why:"YouTube, site hero", job:{ id:"a_1920x1080", prompt:"storm clouds", width:1920, height:1080, frames:97, seed:42, mode:"hero", sweep:0 } },
      { group:"prompt", label:"golden hour light", why:"Warmer grade", job:{ id:"a_golden", prompt:"storm clouds, golden hour light", width:1080, height:1920, frames:97, seed:42, mode:"hero", sweep:0 } },
      { group:"seed", label:"4-seed sweep", why:"Four takes", job:{ id:"a_sweep", prompt:"storm clouds", width:1080, height:1920, frames:97, seed:42, mode:"hero", sweep:4 } },
      { group:"quality", label:"Cheap proof still", why:"Seconds, not an hour", job:{ id:"a_proof", prompt:"storm clouds", width:1080, height:1920, frames:97, seed:42, mode:"test", sweep:0 } },
    ]},
    team: { you:"AIDENWOOD", member:"Aiden", reachable:true, members:[
      { host:"AIDENWOOD", member:"Aiden", model:"Mac16,10", ram_gb:64, role:"coordinator", perf:"full",
        state:"rendering", job:"hero_roof_live", job_prompt:"drone pull-back", elapsed_secs:412,
        done_count:12, is_you:true, app:true, worker:true, gateway:"http://aidenwood.local:8787/?k=abc" },
      { host:"DESK-A", member:"Elijah", model:"Mac15,3", ram_gb:24, role:"worker", perf:"light",
        state:"idle", done_count:4, is_you:false, app:true, worker:false, gateway:"http://desk-a.local:8787/?k=abc" },
      { host:"DESK-C", member:"", model:"Mac14,2", ram_gb:16, role:"worker", perf:"light",
        state:"paused", detail:"paused — low disk", done_count:0, is_you:false, app:false, worker:true, gateway:"" },
    ]},
  },
};

async function newPage(browser, scene, opts){
  opts = opts || {};
  const page = await browser.newPage({
    viewport: opts.viewport || { width:380, height:760 },
    deviceScaleFactor: 2,
  });
  const errs = [];
  // A file:// page has no server, so /poster and /file legitimately fail to load
  // here. Those are verified against the real gateway instead; everything else
  // still fails the run.
  page.on("console", m => {
    if(m.type() !== "error") return;
    if(/Failed to load resource|ERR_CONNECTION_REFUSED|ERR_FILE_NOT_FOUND/.test(m.text())) return;
    errs.push(m.text());
  });
  page.on("pageerror", e => errs.push("PAGEERROR: " + e.message));
  // opts.patch is inlined right after SCENE is defined, so a test can shape the
  // farm (e.g. "no assets on the share") without DOM surgery.
  await page.addInitScript((opts.web ? webBackend : backend)(SCENES[scene], opts.patch || ''));
  await page.goto(webBase);
  await page.waitForTimeout(500);
  page.__errs = errs;
  return page;
}

(async () => {
  const staticServer = await startStaticServer();
  const browser = await chromium.launch();
  if(WANT_SHOTS) fs.mkdirSync(SHOT_DIR, { recursive:true });

  /* ---------------------------------------------------------------- */
  group("boot routing");
  {
    const p = await newPage(browser, "fresh");
    check(await p.locator("#view-wiz").isVisible(), "unconfigured Mac opens on Setup",
      "it opened somewhere else — the original complaint was a dashboard of zeroes");
    check(await p.locator("#wiz-role").isVisible(), "asks the role question first");
    check(!(await p.locator("#wiz-steps").isVisible()), "hides the step list until a role is picked");
    if(WANT_SHOTS) await p.screenshot({ path:`${SHOT_DIR}/01-role.png`, fullPage:true });
    check(p.__errs.length === 0, "no console errors", p.__errs.join(" | "));
    await p.close();
  }
  {
    const p = await newPage(browser, "busy");
    check(await p.locator("#view-dash").isVisible(), "a finished Mac opens on Farm");
    await p.close();
  }

  /* ---------------------------------------------------------------- */
  group("setup wizard");
  {
    const p = await newPage(browser, "coord");
    const steps = p.locator(".step");
    check(await steps.count() === 5, "renders every step", `saw ${await steps.count()}`);
    check(await p.locator(".step.done").count() === 3, "marks finished steps done");
    check(await p.locator(".step.now").count() === 1, "exactly one step is active",
      "more than one active step means the user has two things to decide at once");
    // the active step is the first not-done one
    const activeTitle = await p.locator(".step.now .h").textContent();
    check(activeTitle.includes("toolchain"), "active step is the first unfinished one", activeTitle);
    // finished steps collapse — no body paragraph
    check(await p.locator(".step.done .p").count() === 0,
      "finished steps collapse to one line", "instructions you already followed are noise");
    check(await p.locator(".step.now .p").count() === 1, "the active step explains itself");
    check((await p.locator("#w-count").textContent()).trim() === "3/5", "progress count is right");
    check(await p.locator("#tab-wiz .badge").textContent() === "2", "tab badges the remaining count");
    if(WANT_SHOTS) await p.screenshot({ path:`${SHOT_DIR}/02-coordinator.png`, fullPage:true });
    check(p.__errs.length === 0, "no console errors", p.__errs.join(" | "));
    await p.close();
  }

  /* ---------------------------------------------------------------- */
  group("actions + failure handling");
  {
    const p = await newPage(browser, "coord");
    // success path: the button dispatches the right action
    await p.locator(".step.now .btn.pri").click();
    await p.waitForTimeout(200);
    const calls = await p.evaluate(() => window.__calls.filter(c => c.cmd === "run_action"));
    check(calls.length === 1 && calls[0].args.action === "run_setup",
      "the step button dispatches its own action", JSON.stringify(calls));
    check(await p.locator("#toast.show").isVisible(), "confirms with a toast");
    await p.close();
  }
  {
    const p = await newPage(browser, "coord");
    await p.evaluate(() => { window.__fails["run_action"] = "Can't find the farm scripts"; });
    await p.locator(".step.now .btn.pri").click();
    await p.waitForTimeout(300);
    const err = p.locator(".step.now .err");
    check(await err.count() === 1, "a failure shows INLINE on the step",
      "a toast vanishes; this is the one thing the user must read");
    check((await err.textContent()).includes("Can't find the farm scripts"),
      "the inline error carries the real message");
    const btn = p.locator(".step.now .btn.pri");
    check(await btn.isEnabled(), "the button re-enables after a failure",
      "a permanently disabled button is a dead end");
    check(await btn.textContent() === "Run setup", "the button label is restored");
    if(WANT_SHOTS) await p.screenshot({ path:`${SHOT_DIR}/03-error.png`, fullPage:true });
    await p.close();
  }
  {
    // no double-fire while a slow action is in flight
    const p = await newPage(browser, "coord");
    await p.evaluate(() => {
      const real = window.__TAURI__.core.invoke;
      window.__TAURI__.core.invoke = async (c,a) => {
        if(c === "run_action") { await new Promise(r => setTimeout(r, 600)); }
        return real(c,a);
      };
    });
    const btn = p.locator(".step.now .btn.pri");
    await btn.click();
    await p.waitForTimeout(80);
    check(await btn.isDisabled(), "the button disables while working",
      "an enabled button invites a second click and a duplicate run");
    check((await btn.textContent()).includes("Working"), "and says it's working");
    await p.close();
  }

  /* ---------------------------------------------------------------- */
  group("coordinator discovery");
  {
    const p = await newPage(browser, "worker");
    await p.waitForTimeout(400);
    const hosts = p.locator(".host-btn");
    check(await hosts.count() === 2, "lists the Macs found over Bonjour", `saw ${await hosts.count()}`);
    check((await hosts.first().textContent()).includes("mac-studio"), "shows their names");
    await hosts.first().click();
    await p.waitForTimeout(150);
    const set = await p.evaluate(() => window.__calls.filter(c => c.cmd === "set_coordinator"));
    check(set.length === 1 && set[0].args.name === "mac-studio",
      "picking one sets it — no typing a hostname", JSON.stringify(set));
    if(WANT_SHOTS) await p.screenshot({ path:`${SHOT_DIR}/04-worker.png`, fullPage:true });
    await p.close();
  }

  /* ---------------------------------------------------------------- */
  group("farm view");
  {
    const p = await newPage(browser, "busy");
    await p.waitForTimeout(300);
    check(await p.locator("#c-queued").textContent() === "12", "queued count");
    check(await p.locator("#c-running").textContent() === "3", "rendering count");
    check(await p.locator("#c-failed").textContent() === "2", "failed count");
    check(await p.locator("#ticks .tk").count() === 46, "the lane draws a full set of ticks");
    // every state present in the counts must be visible in the lane
    for(const [cls,label] of [["q","queued"],["r","rendering"],["d","done"],["f","failed"]]){
      check(await p.locator(`#ticks .tk.${cls}`).count() > 0,
        `the lane shows ${label} work`, "a non-zero count with no ticks is a lying chart");
    }
    check(await p.locator(".ev").count() === 4, "activity feed lists the events");
    check(await p.locator("#mach .m").count() === 3,
      "machines are on the Farm view without visiting Checks",
      "which Macs are up is the main thing you open this for");
    check(await p.locator("#mach .m.rendering").count() === 2, "and show which are rendering");
    check((await p.locator("#mach .m").first().textContent()).includes("hero_roof_s1003"),
      "each machine names the job it's on");
    check(await p.locator("#pip.busy").count() === 1, "the header pip shows the farm is busy");
    if(WANT_SHOTS) await p.screenshot({ path:`${SHOT_DIR}/05-farm-busy.png`, fullPage:true });
    check(p.__errs.length === 0, "no console errors", p.__errs.join(" | "));
    await p.close();
  }
  {
    const p = await newPage(browser, "coord");
    await p.evaluate(() => window.__openTab("dash"));
    await p.waitForTimeout(250);
    // scoped to the activity feed: the Team view has its own empty state now
    check(await p.locator("#feed .empty").count() === 1, "an idle farm shows an empty state, not a blank panel");
    check((await p.locator("#feed .empty").textContent()).includes("enqueue.sh"),
      "the empty state says what to do next", "an empty screen is an invitation to act");
    if(WANT_SHOTS) await p.screenshot({ path:`${SHOT_DIR}/06-farm-empty.png`, fullPage:true });
    await p.close();
  }

  /* ---------------------------------------------------------------- */
  group("checks view");
  {
    const p = await newPage(browser, "busy");
    await p.evaluate(() => window.__openTab("checks"));
    await p.waitForTimeout(350);
    check(await p.locator(".chk").count() === 3, "renders each check");
    check(await p.locator(".chk.warn").count() === 1, "colours a warning distinctly");
    check(await p.locator(".chk .fx").count() === 1, "shows the fix text where there is one");
    if(WANT_SHOTS) await p.screenshot({ path:`${SHOT_DIR}/07-checks.png`, fullPage:true });
    check(p.__errs.length === 0, "no console errors", p.__errs.join(" | "));
    await p.close();
  }

  /* ---------------------------------------------------------------- */
  group("resilience");
  {
    // a dead backend must degrade visibly, never white-screen
    const p = await browser.newPage({ viewport:{width:380,height:760} });
    const errs = [];
    p.on("pageerror", e => errs.push(e.message));
    await p.goto(webBase);                       // no __TAURI__ at all
    await p.waitForTimeout(400);
    check(errs.length === 0, "survives with no backend at all", errs.join(" | "));
    check(await p.locator(".top .name").isVisible(), "still renders its chrome");
    await p.close();
  }
  {
    const p = await newPage(browser, "busy");
    await p.evaluate(() => { window.__fails["verify_link"] = "backend exploded"; });
    await p.evaluate(() => window.__openTab("checks"));
    await p.waitForTimeout(300);
    check((await p.locator("#b-title").textContent()).includes("Couldn't run the checks"),
      "a failed verify says so plainly");
    check((await p.locator("#b-sub").textContent()).includes("backend exploded"),
      "and shows the real reason");
    await p.close();
  }
  {
    // tabs must survive being hammered
    const p = await newPage(browser, "busy");
    for(let i=0;i<12;i++) await p.evaluate((n) => window.__openTab(["wiz","dash","checks"][n%3]), i);
    await p.waitForTimeout(300);
    const visible = await p.locator(".view.on").count();
    check(visible === 1, "exactly one view visible after rapid tab switching", `saw ${visible}`);
    check(p.__errs.length === 0, "no console errors under hammering", p.__errs.join(" | "));
    await p.close();
  }

  /* ---------------------------------------------------------------- */
  group("accessibility + polish");
  {
    const p = await newPage(browser, "coord");
    check(await p.locator('[role="tablist"]').count() >= 1, "tabs are a labelled tablist");
    const perList = await p.evaluate(() => [...document.querySelectorAll('[role="tablist"]')]
      .map(l => l.querySelectorAll('[aria-selected="true"]').length));
    check(perList.every(n => n === 1), "exactly one tab is selected in each tablist", JSON.stringify(perList));
    check(await p.locator("#toast[aria-live]").count() === 1, "toasts announce to screen readers");
    // nothing may overflow the popover's fixed width
    const wide = await p.evaluate(() =>
      [...document.querySelectorAll("*")].filter(el => el.scrollWidth > document.documentElement.clientWidth + 1)
        .map(el => el.className || el.tagName).slice(0,5));
    check(wide.length === 0, "nothing overflows 380px", "overflowing: " + wide.join(", "));
    // reduced motion honoured
    const p2 = await browser.newPage({ viewport:{width:380,height:760} });
    await p2.emulateMedia({ reducedMotion:"reduce" });
    await p2.addInitScript(backend(SCENES.busy));
    await p2.goto(webBase);
    await p2.waitForTimeout(300);
    const dur = await p2.evaluate(() => getComputedStyle(document.querySelector(".pip")).animationDuration);
    check(parseFloat(dur) < 0.01, "respects prefers-reduced-motion", `pip animation still ${dur}`);
    await p2.close();
    await p.close();
  }

  /* ---------------------------------------------------------------- */
  group("job board");
  {
    const p = await newPage(browser, "busy");
    await p.evaluate(() => window.__openTab("board"));
    await p.waitForTimeout(350);
    check(await p.locator("#view-board").isVisible(), "Board opens");
    check(await p.locator("#ct-queued").textContent() === "3", "queued lane counts its cards");
    check(await p.locator("#ct-running").textContent() === "1", "rendering lane counts");
    check(await p.locator("#col-queued .jc").count() === 3, "renders a card per queued job");
    const first = p.locator("#col-queued .jc").first();
    check((await first.textContent()).includes("hero_roof_a"), "cards name the job");
    check((await first.textContent()).includes("storm clouds"), "and show the prompt");
    check((await first.textContent()).includes("#1"), "queued cards show their place in the claim order",
      "the whole point of the lane is knowing what renders next");
    check(await p.locator("#col-queued .jc.hi").count() === 1, "the priority job is marked");
    check((await first.textContent()).includes("↓ Normal"),
      "a priority job offers demotion, not another promotion");
    check(await p.locator("#col-running .jc .bar").count() === 1, "the rendering card shows live progress");
    check((await p.locator("#col-running .jc").textContent()).includes("DESK-A"), "and which Mac has it");
    check((await p.locator("#col-done .jc").textContent()).includes("24 MB"), "done cards show the file size");
    check((await p.locator("#col-failed .jc").textContent()).includes("exit 137"), "failed cards show the exit code");
    check(await p.locator("#col-failed .jc .btn").count() >= 2, "a failed job can be requeued from the board");
    if(WANT_SHOTS) await p.screenshot({ path:`${SHOT_DIR}/08-board.png`, fullPage:true });
    check(p.__errs.length === 0, "no console errors", p.__errs.join(" | "));
    await p.close();
  }
  {
    const p = await newPage(browser, "coord");
    await p.evaluate(() => window.__openTab("board"));
    await p.waitForTimeout(300);
    check(await p.locator(".col-empty").count() === 4, "every empty lane says so rather than sitting blank");
    await p.close();
  }
  {
    // queueing a clip: the composer must send a complete, renderable job
    const p = await newPage(browser, "busy");
    await p.evaluate(() => window.__openTab("board"));
    await p.locator("#composer summary").click();
    await p.locator("#n-prompt").fill("hail smashing a skylight, slow motion");
    await p.locator("#n-id").fill("skylight");
    await p.locator("#n-size").selectOption("1920x1080");
    await p.locator("#n-sweep").selectOption("4");
    await p.locator("#n-hi").check();
    await p.locator("#n-go").click();
    await p.waitForTimeout(250);
    const enq = await p.evaluate(() => window.__calls.filter(c => c.cmd === "enqueue_job"));
    check(enq.length === 1, "one submit, one job", JSON.stringify(enq));
    const j = enq[0] && enq[0].args.job || {};
    check(j.prompt === "hail smashing a skylight, slow motion" && j.id === "skylight",
      "carries the prompt and name", JSON.stringify(j));
    check(j.width === 1920 && j.height === 1080, "carries the chosen delivery size");
    check(j.sweep === 4, "carries the seed sweep");
    check(j.priority === "high", "carries the priority lane");
    check(await p.locator("#n-prompt").inputValue() === "", "clears the prompt after queueing");
    await p.close();
  }
  {
    // a prompt is the one required field, and the UI must say so itself
    const p = await newPage(browser, "busy");
    await p.evaluate(() => window.__openTab("board"));
    await p.locator("#composer summary").click();
    await p.locator("#n-go").click();
    await p.waitForTimeout(200);
    check(await p.locator("#toast.bad").isVisible(), "refuses an empty prompt");
    check((await p.evaluate(() => window.__calls.filter(c => c.cmd === "enqueue_job").length)) === 0,
      "and doesn't bother the backend with it");
    await p.close();
  }

  /* ---------------------------------------------------------------- */
  group("reordering what renders next");
  {
    const p = await newPage(browser, "busy");
    await p.evaluate(() => window.__openTab("board"));
    await p.waitForTimeout(300);
    // drag the third queued card to the top
    await p.evaluate(() => {
      const col = document.getElementById("col-queued");
      const cards = [...col.querySelectorAll(".jc")];
      const dt = new DataTransfer();
      cards[2].dispatchEvent(new DragEvent("dragstart", { bubbles:true, dataTransfer:dt }));
      const box = cards[0].getBoundingClientRect();
      cards[0].dispatchEvent(new DragEvent("dragover", { bubbles:true, dataTransfer:dt, clientY: box.top + 2 }));
      cards[2].dispatchEvent(new DragEvent("dragend", { bubbles:true, dataTransfer:dt }));
    });
    await p.waitForTimeout(300);
    const calls = await p.evaluate(() => window.__calls.filter(c => c.cmd === "job_action" && c.args.action === "reorder"));
    check(calls.length === 1, "dropping a card sends one reorder", JSON.stringify(calls));
    check(calls[0] && calls[0].args.order[0] === "s3__c.job",
      "and the dragged job is now first in the claim order", JSON.stringify(calls[0] && calls[0].args.order));
    check(calls[0] && calls[0].args.order.length === 3, "the whole lane order is sent, not just the moved card");
    await p.close();
  }
  {
    const p = await newPage(browser, "busy");
    await p.evaluate(() => window.__openTab("board"));
    await p.waitForTimeout(300);
    await p.locator("#col-queued .jc").nth(1).locator(".btn", { hasText:"↑ Priority" }).click();
    await p.waitForTimeout(200);
    const promo = await p.evaluate(() => window.__calls.filter(c => c.cmd === "job_action" && c.args.action === "promote"));
    check(promo.length === 1 && promo[0].args.file === "s2__b.job", "priority sends the job's own file",
      JSON.stringify(promo));
    await p.close();
  }

  /* ---------------------------------------------------------------- */
  group("variants");
  {
    const p = await newPage(browser, "busy");
    await p.evaluate(() => window.__openTab("board"));
    await p.waitForTimeout(300);
    await p.locator("#col-queued .jc").first().locator(".btn", { hasText:"Variants" }).click();
    await p.waitForTimeout(350);
    check(await p.locator("#panel.open").count() === 1, "variants open in a side panel, not a modal",
      "you need to keep seeing the board while you choose");
    check(await p.locator(".vrow").count() === 5, "offers every recommendation",
      `saw ${await p.locator(".vrow").count()}`);
    check(await p.locator(".vgrp").count() === 4, "grouped by what kind of change it is");
    check(await p.locator(".vrow input:checked").count() === 2,
      "the other delivery sizes are pre-ticked", "that's the request that actually arrives");
    check((await p.locator("#p-count").textContent()).includes("2 selected"), "the count tracks the ticks");
    if(WANT_SHOTS) await p.screenshot({ path:`${SHOT_DIR}/09-variants.png`, fullPage:true });
    await p.locator("#p-queue").click();
    await p.waitForTimeout(400);
    const enq = await p.evaluate(() => window.__calls.filter(c => c.cmd === "enqueue_job"));
    check(enq.length === 2, "queues one job per ticked variant", `sent ${enq.length}`);
    check(enq.every(c => c.args.job && c.args.job.prompt), "each queued variant is a complete job");
    check(enq.some(c => c.args.job.width === 1080 && c.args.job.height === 1080), "including the square one");
    check(await p.locator("#panel.open").count() === 0, "the panel closes once the work is queued");
    check(p.__errs.length === 0, "no console errors", p.__errs.join(" | "));
    await p.close();
  }
  {
    const p = await newPage(browser, "busy");
    await p.evaluate(() => window.__openTab("board"));
    await p.waitForTimeout(250);
    await p.evaluate(() => { window.__fails["job_variants"] = "that job is gone"; });
    await p.locator("#col-done .jc").first().locator(".btn", { hasText:"Variants" }).click();
    await p.waitForTimeout(300);
    check((await p.locator("#p-body").textContent()).includes("that job is gone"),
      "a variant lookup failure is shown in the panel, not swallowed");
    await p.close();
  }

  /* ---------------------------------------------------------------- */
  group("team view");
  {
    const p = await newPage(browser, "busy");
    await p.evaluate(() => window.__openTab("team"));
    await p.waitForTimeout(350);
    check(await p.locator(".mem").count() === 3, "one row per Mac on the farm");
    const mine = p.locator(".mem").first();
    check((await mine.textContent()).includes("Aiden"), "shows the person, not just the hostname");
    check(await mine.locator(".you").count() === 1, "marks which one is you");
    check((await mine.textContent()).includes("hero_roof_live"), "says what their Mac is rendering");
    check((await mine.textContent()).includes("6m 52s"), "and how long it's been at it");
    check((await mine.textContent()).includes("64 GB"), "shows the hardware that matters for renders");
    const last = p.locator(".mem").last();
    check((await last.textContent()).includes("paused — low disk"), "explains a paused Mac");
    check((await p.locator(".mem").nth(1).textContent()).includes("no worker running"),
      "flags a Mac with the app open but no worker running",
      "it looks online but will never claim a job");
    check((await last.textContent()).includes("app not running"),
      "and flags a Mac that isn't running the app",
      "you can't drive that one's setup from here");
    check(await p.locator(".mem a").count() === 0,
      "the popover doesn't offer links to other Macs' gateways", "it can't open them usefully");
    if(WANT_SHOTS) await p.screenshot({ path:`${SHOT_DIR}/10-team.png`, fullPage:true });
    check(p.__errs.length === 0, "no console errors", p.__errs.join(" | "));
    await p.close();
  }
  {
    const p = await newPage(browser, "busy");
    await p.evaluate(() => window.__openTab("team"));
    await p.waitForTimeout(300);
    await p.locator("#t-name").fill("Aiden W");
    await p.locator("#t-save").click();
    await p.waitForTimeout(200);
    const set = await p.evaluate(() => window.__calls.filter(c => c.cmd === "set_member"));
    check(set.length === 1 && set[0].args.name === "Aiden W", "saving your name reaches the backend",
      JSON.stringify(set));
    await p.close();
  }

  /* ---------------------------------------------------------------- */
  group("web gateway surface");
  {
    const p = await newPage(browser, "busy", { web:true, viewport:{ width:1280, height:900 } });
    check(await p.evaluate(() => document.body.classList.contains("web")),
      "the browser surface gets the wider layout");
    check(await p.locator("#view-board").isVisible(), "a browser opens straight onto the board",
      "that's what you open the link for");
    const cols = await p.evaluate(() => getComputedStyle(document.getElementById("board")).gridTemplateColumns.split(" ").length);
    check(cols === 4, "the board is four columns on a desktop browser", `saw ${cols}`);
    check((await p.evaluate(() => window.__calls.length)) > 0, "it talks to the gateway over HTTP");
    check((await p.locator("#col-done .jc").textContent()).includes("Watch"),
      "a finished clip can be watched in the browser", "no SMB mount needed to review a render");
    const dl = await p.locator("#col-done .jc a").first().getAttribute("href");
    check(dl && dl.includes("/file?path=") && dl.includes("dl=1"), "and downloaded straight off the share", dl);
    if(WANT_SHOTS) await p.screenshot({ path:`${SHOT_DIR}/11-web-board.png`, fullPage:true });
    check(p.__errs.length === 0, "no console errors", p.__errs.join(" | "));
    await p.close();
  }
  {
    const p = await newPage(browser, "busy", { web:true, viewport:{ width:1280, height:900 } });
    await p.evaluate(() => window.__openTab("team"));
    await p.waitForTimeout(300);
    check(await p.locator(".mem a").count() === 1,
      "in a browser you can hop to the gateway of a Mac that's sharing one",
      "that's how you finish setting up Mac 3 from Mac 1");
    const href = await p.locator(".mem a").first().getAttribute("href");
    check(href === "http://desk-a.local:8787/?k=abc", "and the link carries their key", href);
    check((await p.locator(".mem").last().locator("a").count()) === 0,
      "a Mac that isn't sharing a gateway gets no dead link");
    await p.close();
  }
  {
    // the phone: a bottom sheet, no horizontal scroll, still usable
    const p = await newPage(browser, "busy", { web:true, viewport:{ width:390, height:844 } });
    await p.evaluate(() => window.__openTab("board"));
    await p.waitForTimeout(300);
    const wide = await p.evaluate(() =>
      [...document.querySelectorAll("*")].filter(el => el.scrollWidth > document.documentElement.clientWidth + 1)
        .map(el => el.className || el.tagName).slice(0,5));
    check(wide.length === 0, "nothing overflows a phone screen", "overflowing: " + wide.join(", "));
    await p.locator("#col-queued .jc").first().locator(".btn", { hasText:"Variants" }).click();
    await p.waitForTimeout(300);
    const t = await p.evaluate(() => getComputedStyle(document.getElementById("panel")).borderTopLeftRadius);
    check(parseFloat(t) > 8, "the panel becomes a bottom sheet on a phone", `radius ${t}`);
    if(WANT_SHOTS) await p.screenshot({ path:`${SHOT_DIR}/12-web-phone.png`, fullPage:true });
    await p.close();
  }
  {
    // a browser opened without the key must say what's wrong, in plain words
    const p = await browser.newPage({ viewport:{ width:900, height:700 } });
    await p.addInitScript(`
      window.fetch = async () => ({ ok:false, status:403, json: async () => ({ ok:false, error:"nope" }) });
    `);
    const errs = [];
    p.on("pageerror", e => errs.push(e.message));
    await p.goto(webBase);
    await p.waitForTimeout(500);
    await p.evaluate(() => window.__openTab("checks"));
    await p.waitForTimeout(300);
    check(errs.length === 0, "a rejected gateway never white-screens", errs.join(" | "));
    check((await p.locator("#b-sub").textContent()).includes("missing its key"),
      "and explains that the link needs its key", await p.locator("#b-sub").textContent());
    await p.close();
  }

  /* ---------------------------------------------------------------- */
  group("gateway settings");
  {
    const p = await newPage(browser, "busy");
    await p.evaluate(() => window.__openTab("checks"));
    await p.waitForTimeout(400);
    check((await p.locator("#w-url").textContent()).includes("aidenwood.local:8787"),
      "shows the link the team should use when LAN sharing is on");
    check(await p.locator("#w-warn").isVisible(),
      "and warns plainly about what LAN sharing means",
      "this one is worth spelling out — it lets other people drive this Mac");
    check(await p.locator("#w-on").isChecked(), "reflects that the gateway is running");
    check(await p.locator("#w-auto").isChecked(), "reflects the open-on-launch setting");
    await p.locator("#w-port").fill("9100");
    await p.locator('#webcfg button[type="submit"]').click();
    await p.waitForTimeout(250);
    const saves = await p.evaluate(() => window.__calls.filter(c => c.cmd === "save_config"));
    check(saves.length === 1, "applying sends one save", JSON.stringify(saves));
    const sent = saves[0] && saves[0].args.cfg || {};
    check(sent.web_port === 9100, "with the new port", JSON.stringify(sent));
    check(Object.keys(sent).length === 4,
      "and only the gateway's own fields — the backend merges the rest",
      "sending a partial config used to wipe the role and reopen the wizard: " + JSON.stringify(sent));
    await p.close();
  }
  {
    // untick LAN sharing and the warning must go away immediately
    const p = await newPage(browser, "busy");
    await p.evaluate(() => window.__openTab("checks"));
    await p.waitForTimeout(400);
    await p.locator("#w-lan").uncheck();
    check(!(await p.locator("#w-warn").isVisible()), "the LAN warning tracks the checkbox");
    await p.close();
  }

  /* ---------------------------------------------------------------- */
  group("both surfaces stay in step");
  {
    const p = await newPage(browser, "busy");
    await p.evaluate(() => window.__openTab("checks"));
    await p.waitForTimeout(400);
    const before = await p.evaluate(() => window.__calls.filter(c => c.cmd === "get_config").length);
    // something changed a setting elsewhere — another tab, the popover, a phone
    await p.evaluate(() => { window.__rev = 99; });
    await p.waitForTimeout(2600);
    const after = await p.evaluate(() => window.__calls.filter(c => c.cmd === "get_config").length);
    check(after > before, "a change made on another surface pulls a fresh config within one poll",
      `get_config calls: ${before} -> ${after}`);
    check(p.__errs.length === 0, "no console errors", p.__errs.join(" | "));
    await p.close();
  }

  /* ---------------------------------------------------------------- */
  group("board at scale");
  {
    const p = await newPage(browser, "busy", { web:true, viewport:{ width:1280, height:900 } });
    await p.waitForTimeout(400);
    // search
    await p.locator("#b-search").fill("driveway");
    await p.waitForTimeout(250);
    check(await p.locator("#col-queued .jc").count() === 1, "search filters the lanes",
      `saw ${await p.locator("#col-queued .jc").count()}`);
    check((await p.locator("#ct-queued").textContent()) === "1/3",
      "and says how many of the total are showing", await p.locator("#ct-queued").textContent());
    await p.locator("#f-clear").click();
    await p.waitForTimeout(250);
    check(await p.locator("#col-queued .jc").count() === 3, "clearing puts them back");

    // a filter with no matches must explain itself, not look broken
    await p.locator("#b-search").fill("nothinglikethis");
    await p.waitForTimeout(250);
    check((await p.locator("#col-queued .col-empty").textContent()).includes("matches the filter"),
      "an empty lane says it's the filter, not an empty farm");
    await p.locator("#f-clear").click();
    await p.waitForTimeout(200);

    // filter by Mac and by run
    await p.locator("#f-lane-host").selectOption("DESK-C");
    await p.waitForTimeout(250);
    check(await p.locator("#col-failed .jc").count() === 1 && await p.locator("#col-queued .jc").count() === 0,
      "filtering by Mac narrows every lane at once");
    await p.locator("#f-clear").click();
    await p.waitForTimeout(200);
    check(await p.locator("#f-lane-run option").count() >= 2, "the run filter is built from what's on the board");
    check(p.__errs.length === 0, "no console errors", p.__errs.join(" | "));
    await p.close();
  }
  {
    const p = await newPage(browser, "busy", { web:true, viewport:{ width:1280, height:900 } });
    await p.waitForTimeout(400);
    check(await p.locator("#bulk").isHidden(), "the bulk bar stays out of the way until you select something");
    await p.locator("#col-queued .jc").nth(0).locator(".pick").click();
    await p.locator("#col-queued .jc").nth(1).locator(".pick").click();
    await p.waitForTimeout(200);
    check(await p.locator("#bulk").isVisible(), "selecting shows the bulk bar");
    check((await p.locator("#bulk-n").textContent()).includes("2 selected"), "with a count");
    await p.locator("#bulk-acts .btn", { hasText:"Remove" }).click();
    await p.waitForTimeout(350);
    const calls = await p.evaluate(() => window.__calls.filter(c => c.cmd === "job_action" && c.args.action === "cancel"));
    check(calls.length === 1, "one call for the whole selection, not one per card", JSON.stringify(calls));
    check(calls[0] && calls[0].args.files && calls[0].args.files.length === 2,
      "and it carries both files", JSON.stringify(calls[0] && calls[0].args));
    check(await p.locator("#bulk").isHidden(), "the selection clears after acting");
    await p.close();
  }
  {
    // mixed lanes must not offer an action that only works for one of them
    const p = await newPage(browser, "busy", { web:true, viewport:{ width:1280, height:900 } });
    await p.waitForTimeout(400);
    await p.locator("#col-queued .jc").first().locator(".pick").click();
    await p.locator("#col-failed .jc").first().locator(".pick").click();
    await p.waitForTimeout(200);
    check((await p.locator("#bulk-acts").textContent()).includes("one lane at a time"),
      "a mixed selection says so instead of offering a half-working button");
    check(await p.locator("#bulk-acts .btn").count() === 0, "and offers no actions");
    await p.close();
  }
  {
    // keyboard: select the queued lane and promote it
    const p = await newPage(browser, "busy", { web:true, viewport:{ width:1280, height:900 } });
    await p.waitForTimeout(400);
    await p.keyboard.press("a");
    await p.waitForTimeout(250);
    check((await p.locator("#bulk-n").textContent()).includes("3 selected"), "‘a’ selects the queued lane");
    await p.keyboard.press("p");
    await p.waitForTimeout(350);
    const promo = await p.evaluate(() => window.__calls.filter(c => c.cmd === "job_action" && c.args.action === "promote"));
    check(promo.length === 1 && promo[0].args.files.length === 3, "‘p’ promotes the selection", JSON.stringify(promo));
    await p.keyboard.press("4");
    await p.waitForTimeout(250);
    check(await p.locator("#view-review").isVisible(), "number keys jump between views");
    await p.close();
  }

  /* ---------------------------------------------------------------- */
  group("cards carry the new facts");
  {
    const p = await newPage(browser, "busy", { web:true, viewport:{ width:1280, height:900 } });
    await p.waitForTimeout(400);
    const first = p.locator("#col-queued .jc").first();
    const txt = await first.textContent();
    check(txt.includes("by Aiden"), "queued cards say who asked for the render", txt);
    check(txt.includes("overnight"), "and which run it belongs to");
    check(txt.includes("to render"), "and roughly how long it takes on this farm", txt);
    const second = await p.locator("#col-queued .jc").nth(1).textContent();
    check(second.includes("starts in"), "a job behind others says when it should start", second);
    const running = await p.locator("#col-running .jc").textContent();
    check(running.includes("left") || running.includes("running long"),
      "a rendering card says how much longer", running);
    const failed = await p.locator("#col-failed .jc").textContent();
    check(failed.includes("Bigger Mac") && failed.includes("Smaller"),
      "a memory kill offers the two answers that actually help", failed);
    check(await p.locator("#col-done .jc .poster").count() === 1,
      "a finished clip shows a poster frame, not just a filename");
    check(await p.locator("#held-banner").isVisible(), "a held queue says so");
    check((await p.locator("#held-sub").textContent()).includes("3 job(s) held"), "with the number held");
    await p.close();
  }
  {
    const p = await newPage(browser, "busy", { web:true, viewport:{ width:1280, height:900 } });
    await p.waitForTimeout(400);
    await p.locator("#col-done .jc .btn", { hasText:"Approve" }).first().click();
    await p.waitForTimeout(300);
    const rev = await p.evaluate(() => window.__calls.filter(c => c.cmd === "set_review"));
    check(rev.length === 1 && rev[0].args.state === "approved" && rev[0].args.id === "hero_roof_fin",
      "approving a clip from the board reaches the backend", JSON.stringify(rev));
    await p.close();
  }
  {
    // the log panel works on both surfaces and shows the real step counter
    const p = await newPage(browser, "busy");
    await p.evaluate(() => window.__openTab("board"));
    await p.waitForTimeout(350);
    await p.locator("#col-running .jc .btn", { hasText:"Log" }).click();
    await p.waitForTimeout(400);
    check(await p.locator("#panel.open").count() === 1, "the log opens in the panel");
    check((await p.locator("#p-body").textContent()).includes("step 12 of 40"),
      "and shows the renderer's own progress, not a guess", await p.locator("#p-body").textContent());
    await p.locator("#p-close").click();
    await p.close();
  }

  /* ---------------------------------------------------------------- */
  group("review — the cherry-pick loop");
  {
    const p = await newPage(browser, "busy", { web:true, viewport:{ width:1280, height:900 } });
    await p.evaluate(() => window.__openTab("review"));
    await p.waitForTimeout(400);
    check(await p.locator("#rv-proofs .tile").count() === 2, "proof stills are laid out as a contact sheet");
    const done = p.locator("#rv-proofs .tile").nth(1);
    check((await done.textContent()).includes("hero rendered"),
      "a still that already has a hero render says so", "otherwise you render it twice");
    check(await done.locator(".btn", { hasText:"Render hero" }).count() === 0,
      "and doesn't offer to render it again");
    const fresh = p.locator("#rv-proofs .tile").first();
    check(await fresh.locator(".btn", { hasText:"Render hero" }).count() === 1,
      "a fresh still offers the full render — that IS the cherry-pick loop");
    await fresh.locator(".btn", { hasText:"Render hero" }).click();
    await p.waitForTimeout(300);
    const hero = await p.evaluate(() => window.__calls.filter(c => c.cmd === "job_action" && c.args.action === "render_hero"));
    check(hero.length === 1 && hero[0].args.file === "s7__proof_a.job.DESK-A.9.ok",
      "and it points at the right job record", JSON.stringify(hero));
    check(await p.locator("#rv-proofs .tile.approved").count() === 1, "an approved still is marked");
    if(WANT_SHOTS) await p.screenshot({ path:`${SHOT_DIR}/13-review.png`, fullPage:true });
    check(p.__errs.length === 0, "no console errors", p.__errs.join(" | "));
    await p.close();
  }
  {
    const p = await newPage(browser, "busy", { web:true, viewport:{ width:1280, height:900 } });
    await p.evaluate(() => window.__openTab("review"));
    await p.waitForTimeout(400);
    await p.locator("#rtab-clips").click();
    await p.waitForTimeout(300);
    check(await p.locator("#rv-clips .tile").count() === 1, "finished clips get their own grid");
    check(await p.locator("#rv-proofs").isHidden(), "and the stills step aside");
    await p.locator("#rv-clips .tile .btn", { hasText:"Needs another" }).click();
    await p.waitForTimeout(300);
    const rev = await p.evaluate(() => window.__calls.filter(c => c.cmd === "set_review"));
    check(rev.length === 1 && rev[0].args.state === "retake", "sending a clip back works from the grid",
      JSON.stringify(rev));
    await p.close();
  }

  /* ---------------------------------------------------------------- */
  group("overnight run planner");
  {
    const p = await newPage(browser, "busy", { web:true, viewport:{ width:1280, height:900 } });
    await p.waitForTimeout(400);
    await p.locator("#planner summary").click();
    await p.locator("#r-prompts").fill("a roof\na gutter\n\na driveway");
    await p.locator("#r-sizes input[value='1920x1080']").check();
    await p.locator("#r-seeds").selectOption("2");
    await p.waitForTimeout(250);
    const sum = await p.locator("#r-sum").textContent();
    check(sum.includes("3 prompt(s)") && sum.includes("2 size(s)") && sum.includes("2 take(s)"),
      "the planner counts the night before you commit it", sum);
    check(sum.includes("12 job(s)"), "3 × 2 × 2 = 12", sum);
    check(/roughly/.test(sum) && /Mac\(s\)/.test(sum), "and says roughly how long that is across the farm", sum);

    await p.locator("#r-name").fill("overnight");
    await p.locator("#r-go").click();
    await p.waitForTimeout(400);
    const plan = await p.evaluate(() => window.__calls.filter(c => c.cmd === "plan_run"));
    check(plan.length === 1, "one submit, one run", JSON.stringify(plan));
    const pl = plan[0] && plan[0].args.plan || {};
    check(pl.prompts.filter(x => x.trim()).length === 3, "blank lines are dropped", JSON.stringify(pl.prompts));
    check(pl.sizes.length === 2 && pl.seeds === 2, "sizes and takes carry over", JSON.stringify(pl));
    check(pl.run === "overnight", "and the run keeps its name");
    check(await p.locator("#r-prompts").inputValue() === "", "the shot list clears once it's queued");
    if(WANT_SHOTS) await p.screenshot({ path:`${SHOT_DIR}/14-planner.png`, fullPage:true });
    await p.close();
  }
  {
    const p = await newPage(browser, "busy", { web:true, viewport:{ width:1280, height:900 } });
    await p.waitForTimeout(400);
    await p.locator("#planner summary").click();
    await p.locator("#r-mode").selectOption("test");
    await p.locator("#r-prompts").fill("a roof");
    await p.waitForTimeout(250);
    check((await p.locator("#r-sum").textContent()).includes("proof stills"),
      "planning proofs says it's the cheap option");
    await p.locator("#r-go").click();
    await p.waitForTimeout(300);
    const pl = (await p.evaluate(() => window.__calls.filter(c => c.cmd === "plan_run")))[0].args.plan;
    check(pl.mode === "test", "and the mode reaches the backend");
    await p.close();
  }
  {
    const p = await newPage(browser, "busy", { web:true, viewport:{ width:1280, height:900 } });
    await p.waitForTimeout(400);
    await p.locator("#planner summary").click();
    await p.locator("#r-go").click();
    await p.waitForTimeout(250);
    check(await p.locator("#toast.bad").isVisible(), "an empty shot list is refused locally");
    check((await p.evaluate(() => window.__calls.filter(c => c.cmd === "plan_run").length)) === 0,
      "without bothering the farm");
    await p.close();
  }

  /* ---------------------------------------------------------------- */
  group("runs + the morning report");
  {
    const p = await newPage(browser, "busy", { web:true, viewport:{ width:1280, height:900 } });
    await p.evaluate(() => window.__openTab("dash"));
    await p.waitForTimeout(500);
    check(await p.locator("#runs .run").count() === 2, "runs are listed on the Farm view");
    check(await p.locator("#runs .run.finished").count() === 1, "a finished run is marked");
    const live = await p.locator("#runs .run").nth(1).textContent();
    check(live.includes("waiting") && live.includes("rendering"), "an in-flight run shows where it's up to", live);
    await p.locator("#runs .run").first().locator(".btn", { hasText:"Report" }).click();
    await p.waitForTimeout(400);
    check(await p.locator("#panel.open").count() === 1, "the report opens in the panel");
    const rep = await p.locator("#p-body").textContent();
    check(rep.includes("12 done") && rep.includes("2 failed"), "and adds up the night", rep);
    check(rep.includes("3 approved"), "including what's been reviewed", rep);
    check(rep.includes("Failed — worth a look"), "it puts the failures where you'll act on them");
    check(await p.locator("#p-body .jc").count() >= 1, "with the failed cards themselves, actionable");
    if(WANT_SHOTS) await p.screenshot({ path:`${SHOT_DIR}/15-run-report.png`, fullPage:true });
    check(p.__errs.length === 0, "no console errors", p.__errs.join(" | "));
    await p.close();
  }
  {
    // The Farm view can be the first thing you open, so it must load runs itself
    // rather than relying on the Board having been visited.
    const p = await newPage(browser, "busy", { web:true, viewport:{ width:1280, height:900 } });
    await p.evaluate(() => window.__openTab("dash"));
    await p.waitForTimeout(600);
    check(await p.locator("#runs .run").count() === 2, "runs load on the Farm view on their own",
      `saw ${await p.locator("#runs .run").count()}`);
    check((await p.evaluate(() => window.__calls.filter(c => c.cmd === "get_runs").length)) > 0,
      "it asks for them directly");
    await p.close();
  }
  {
    const p = await newPage(browser, "busy", { web:true, viewport:{ width:1280, height:900 } });
    await p.evaluate(() => window.__openTab("dash"));
    await p.waitForTimeout(500);
    const st = await p.locator("#stats").textContent();
    check(st.includes("42"), "the farm's own numbers are on the Farm view", st);
    check(st.includes("Per Mac") && st.includes("AIDENWOOD"), "broken down per Mac", st);
    check(st.includes("By size"), "and per delivery size", st);
    check((await p.locator("#stats .v.warn").count()) === 1,
      "renders that blew their memory budget are flagged", "that's the number that retunes farm.conf");
    await p.close();
  }

  /* ---------------------------------------------------------------- */
  group("image-to-video + presets");
  {
    const p = await newPage(browser, "busy", { web:true, viewport:{ width:1280, height:900 } });
    await p.waitForTimeout(400);
    await p.locator("#composer summary").click();
    check(await p.locator("#wrap-image").isHidden(), "a text job doesn't ask for an image");
    await p.locator("#n-kind").selectOption("i2v");
    await p.waitForTimeout(200);
    check(await p.locator("#wrap-image").isVisible(), "an image job does");
    check(await p.locator("#n-image option").count() === 2, "and lists what's already in assets/");
    check(await p.locator("#wrap-lora").isHidden(), "but doesn't ask about LoRAs yet");
    await p.locator("#n-kind").selectOption("lora_i2v");
    await p.waitForTimeout(200);
    check(await p.locator("#wrap-lora").isVisible() && await p.locator("#wrap-still").isVisible(),
      "a LoRA job asks for the LoRA and the still prompt");
    check((await p.locator("#n-lora option").first().textContent()).includes("Elijah"), "listing the LoRAs on the share");

    await p.close();
  }
  {
    // an i2v job with no image must be refused before it reaches the farm.
    // The farm here has nothing in assets/, which is the real way to get there.
    const p = await newPage(browser, "busy", { web:true, viewport:{ width:1280, height:900 },
      patch: `SCENE.assets = { images: [], loras: [] };` });
    await p.waitForTimeout(400);
    await p.locator("#composer summary").click();
    await p.locator("#n-kind").selectOption("i2v");
    await p.locator("#n-prompt").fill("he lifts the glass");
    await p.locator("#n-go").click();
    await p.waitForTimeout(250);
    check((await p.evaluate(() => window.__calls.filter(c => c.cmd === "enqueue_job").length)) === 0,
      "an image job with no image is refused locally");
    check(await p.locator("#toast.bad").isVisible(), "and says why");
    await p.close();
  }
  {
    const p = await newPage(browser, "busy", { web:true, viewport:{ width:1280, height:900 } });
    await p.waitForTimeout(400);
    await p.locator("#composer summary").click();
    await p.locator("#n-size").selectOption("1920x1080");
    await p.locator("#n-sweep").selectOption("4");
    p.on("dialog", d => d.accept("wide sweep"));
    await p.locator("#n-save-preset").click();
    await p.waitForTimeout(300);
    const saved = await p.evaluate(() => window.__calls.filter(c => c.cmd === "save_preset"));
    check(saved.length === 1 && saved[0].args.name === "wide sweep", "a setup can be saved as a preset",
      JSON.stringify(saved));
    check(saved[0].args.job.width === 1920 && saved[0].args.job.sweep === 4, "with its shape");
    check(saved[0].args.job.prompt === "", "but not the prompt — a preset is a shape, not a shot");
    check(await p.locator("#wrap-presets").isVisible(), "and it appears in the picker");
    await p.close();
  }

  /* ---------------------------------------------------------------- */
  group("ops + autopilot + farm-wide limits");
  {
    const p = await newPage(browser, "busy", { web:true, viewport:{ width:1280, height:1000 } });
    await p.evaluate(() => window.__openTab("checks"));
    await p.waitForTimeout(600);
    check(await p.locator("#a-on").isChecked(), "the autopilot toggle reflects reality");
    check((await p.locator("#a-status").textContent()).includes("watching the farm"),
      "and says plainly what it's doing", await p.locator("#a-status").textContent());
    check((await p.locator("#op-resume-sub").textContent()).includes("3 job(s) are held"),
      "the resume button says how many jobs are waiting on it");
    await p.locator("#a-log").click();
    await p.waitForTimeout(300);
    check((await p.locator("#p-body").textContent()).includes("requeued 2 stalled"),
      "autopilot's diary is readable — an unattended system has to explain itself");
    await p.locator("#p-close").click();

    await p.locator("#a-retry").selectOption("2");
    await p.locator('#autocfg button[type="submit"]').click();
    await p.waitForTimeout(300);
    const set = await p.evaluate(() => window.__calls.filter(c => c.cmd === "set_autopilot"));
    check(set.length === 1 && set[0].args.retry === 2, "the policy saves", JSON.stringify(set));

    await p.locator("#op-reap").click();
    await p.waitForTimeout(300);
    const reap = await p.evaluate(() => window.__calls.filter(c => c.cmd === "farm_action" && c.args.action === "reap"));
    check(reap.length === 1, "reaping stalled jobs is one button now, not a Terminal command");
    if(WANT_SHOTS) await p.screenshot({ path:`${SHOT_DIR}/16-ops.png`, fullPage:true });
    check(p.__errs.length === 0, "no console errors", p.__errs.join(" | "));
    await p.close();
  }
  {
    const p = await newPage(browser, "busy", { web:true, viewport:{ width:1280, height:1000 } });
    await p.evaluate(() => window.__openTab("checks"));
    await p.waitForTimeout(600);
    check(await p.locator("#conf-fields .f").count() === 3, "farm.conf is editable field by field");
    check(await p.locator("#cf-ADMISSION option").count() === 2, "a choice key is a dropdown, not free text",
      "free text in a bash file is how you break every worker at once");
    check(await p.evaluate(() => document.getElementById("cf-MEM_BUDGET_PCT").type) === "number",
      "a numeric key is a number field");
    // nothing changed -> nothing sent
    await p.locator('#confcfg button[type="submit"]').click();
    await p.waitForTimeout(250);
    check((await p.evaluate(() => window.__calls.filter(c => c.cmd === "save_farm_conf").length)) === 0,
      "applying with no changes doesn't rewrite the farm's config");
    await p.locator("#cf-MEM_BUDGET_PCT").fill("80");
    await p.locator('#confcfg button[type="submit"]').click();
    await p.waitForTimeout(300);
    const saved = await p.evaluate(() => window.__calls.filter(c => c.cmd === "save_farm_conf"));
    check(saved.length === 1, "a change is sent", JSON.stringify(saved));
    check(Object.keys(saved[0].args.keys).length === 1 && saved[0].args.keys.MEM_BUDGET_PCT === "80",
      "and only the key that changed", JSON.stringify(saved[0].args.keys));
    await p.close();
  }

  await browser.close();
  staticServer.close();
  console.log(`\n\x1b[1m${pass} passed · ${fail} failed\x1b[0m`);
  if(WANT_SHOTS) console.log(`screenshots: ${SHOT_DIR}`);
  if(fail){ console.log("\x1b[31m" + failures.map(f => "  · " + f).join("\n") + "\x1b[0m"); process.exit(1); }
  console.log("\x1b[32mUI behaves.\x1b[0m");
})().catch(e => { console.error(e); process.exit(1); });
