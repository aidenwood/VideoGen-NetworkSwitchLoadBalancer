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

const UI = "file://" + path.resolve(__dirname, "../ui/index.html");
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
   so tests can assert on dispatch, and `fails` forces a command to reject. */
function bridge(scene){
  return `
    window.__calls = [];
    window.__fails = {};
    const SCENE = ${JSON.stringify(scene)};
    window.__TAURI__ = { core: { invoke: async (cmd, args) => {
      window.__calls.push({ cmd, args });
      if (window.__fails[cmd]) throw new Error(window.__fails[cmd]);
      switch(cmd){
        case "setup_steps": return SCENE.setup;
        case "get_state":   return SCENE.state;
        case "verify_link": return SCENE.verify;
        case "get_config":  return SCENE.config;
        case "discover_coordinators": return SCENE.hosts || [];
        case "run_action":  return "ran " + args.action;
        case "set_role":       SCENE.setup.role = args.role; return null;
        case "set_coordinator":return null;
        case "finish_wizard":  return null;
        case "save_config":    return null;
        case "pick_repo":      return "Farm scripts: /somewhere";
        default: return null;
      }
    }}};
  `;
}

const step = (o) => Object.assign(
  { id:"x", title:"Step", body:"Body text.", done:false, detail:"detail",
    action:"", action_label:"", manual:false }, o);

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
    config:{ config:{coordinator:"AIDENWOOD",share_name:"RenderFarm",perf:"auto",min_free_gb:15}, resolved:{root:"/Users/aidenwood/RenderFarm"}, config_file:"~/…/config.json" },
  },
};

async function newPage(browser, scene){
  const page = await browser.newPage({ viewport:{width:380,height:760}, deviceScaleFactor:2 });
  const errs = [];
  page.on("console", m => { if(m.type() === "error") errs.push(m.text()); });
  page.on("pageerror", e => errs.push("PAGEERROR: " + e.message));
  await page.addInitScript(bridge(SCENES[scene]));
  await page.goto(UI);
  await page.waitForTimeout(500);
  page.__errs = errs;
  return page;
}

(async () => {
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
    check(await p.locator(".empty").count() === 1, "an idle farm shows an empty state, not a blank panel");
    check((await p.locator(".empty").textContent()).includes("enqueue.sh"),
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
    await p.goto(UI);                       // no __TAURI__ at all
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
    check(await p.locator('[role="tablist"]').count() === 1, "tabs are a labelled tablist");
    check(await p.locator('[aria-selected="true"]').count() === 1, "exactly one tab is selected");
    check(await p.locator("#toast[aria-live]").count() === 1, "toasts announce to screen readers");
    // nothing may overflow the popover's fixed width
    const wide = await p.evaluate(() =>
      [...document.querySelectorAll("*")].filter(el => el.scrollWidth > document.documentElement.clientWidth + 1)
        .map(el => el.className || el.tagName).slice(0,5));
    check(wide.length === 0, "nothing overflows 380px", "overflowing: " + wide.join(", "));
    // reduced motion honoured
    const p2 = await browser.newPage({ viewport:{width:380,height:760} });
    await p2.emulateMedia({ reducedMotion:"reduce" });
    await p2.addInitScript(bridge(SCENES.busy));
    await p2.goto(UI);
    await p2.waitForTimeout(300);
    const dur = await p2.evaluate(() => getComputedStyle(document.querySelector(".pip")).animationDuration);
    check(parseFloat(dur) < 0.01, "respects prefers-reduced-motion", `pip animation still ${dur}`);
    await p2.close();
    await p.close();
  }

  await browser.close();
  console.log(`\n\x1b[1m${pass} passed · ${fail} failed\x1b[0m`);
  if(WANT_SHOTS) console.log(`screenshots: ${SHOT_DIR}`);
  if(fail){ console.log("\x1b[31m" + failures.map(f => "  · " + f).join("\n") + "\x1b[0m"); process.exit(1); }
  console.log("\x1b[32mUI behaves.\x1b[0m");
})().catch(e => { console.error(e); process.exit(1); });
