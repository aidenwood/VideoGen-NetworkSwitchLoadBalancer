/* Checks — the live per-step verdict for THIS Mac, plus every setting: the share,
   the overnight autopilot, farm-wide limits, farm operations, and the gateway. */
import { useCallback, useEffect, useState } from 'react';
import { call, errText } from '../api';
import { Btn, usePanel } from '../ui';
import type {
  AutopilotResponse,
  Check,
  ConfKey,
  ConfigResponse,
  FarmConfResponse,
  VerifyReport,
} from '../types';

type Show = (m: string, k?: 'good' | 'bad') => void;

const MARK: Record<string, string> = { ok: '✅', warn: '⚠️', fail: '❌' };

export default function Checks({
  active,
  config,
  onConfig,
  show,
}: {
  active: boolean;
  config: ConfigResponse | null;
  onConfig: () => void;
  show: Show;
}) {
  const panel = usePanel();
  const [report, setReport] = useState<VerifyReport | null>(null);
  const [verifyErr, setVerifyErr] = useState('');
  const [verifying, setVerifying] = useState(false);

  const verify = useCallback(async () => {
    if (verifying) return;
    setVerifying(true);
    try {
      setReport(await call<VerifyReport>('verify_link'));
      setVerifyErr('');
    } catch (e) {
      setVerifyErr(errText(e));
    } finally {
      setVerifying(false);
    }
  }, [verifying]);

  useEffect(() => {
    if (!active) return;
    void verify();
    const t = setInterval(() => {
      if (document.hasFocus()) void verify();
    }, 15000);
    return () => clearInterval(t);
    // verify is stable in practice; re-creating the interval on each render would
    // reset the 15s cadence.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active]);

  const groups: { label: string; items: Check[] }[] = [];
  for (const c of report?.checks ?? []) {
    const last = groups[groups.length - 1];
    if (!last || last.label !== c.stage_label) groups.push({ label: c.stage_label, items: [c] });
    else last.items.push(c);
  }

  const banner = !report
    ? { cls: '', icon: verifyErr ? '❌' : '⏳', title: verifyErr ? "Couldn't run the checks" : 'Checking this Mac…', sub: verifyErr }
    : {
        cls: report.fail ? 'blocked' : report.warn ? '' : 'ready',
        icon: report.fail ? '❌' : report.warn ? '⚠️' : '✅',
        title: report.fail
          ? `${report.fail} ${report.fail === 1 ? 'step' : 'steps'} left before this Mac can render`
          : report.warn
            ? `Connected — ${report.warn} worth a look`
            : 'This Mac is fully set up',
        sub: `${report.host}${report.is_coordinator ? ' · coordinator' : ''} · ${report.root}`,
      };

  return (
    <section aria-labelledby="tab-checks" className={`view${active ? ' on' : ''}`} id="view-checks" role="tabpanel">
      <div className={`banner ${banner.cls}`} id="banner">
        <span className="g" id="b-icon">
          {banner.icon}
        </span>
        <span className="tx">
          <b id="b-title">{banner.title}</b>
          <span id="b-sub">{banner.sub}</span>
        </span>
        <Btn id="b-recheck" label="Re-check" onClick={verify} />
      </div>

      <div id="checks">
        {groups.map((g) => (
          <div className="grp" key={g.label}>
            <h2>{g.label}</h2>
            {g.items.map((c) => (
              <CheckRow c={c} key={c.id} onDone={verify} show={show} />
            ))}
          </div>
        ))}
      </div>

      <h2>Settings</h2>
      <ShareSettings config={config} onSaved={() => { onConfig(); void verify(); }} show={show} />
      <div className="hint" id="cfgfile" style={{ marginTop: 9 }}>
        {config?.config_file ? `Saved to ${config.config_file}` : ''}
      </div>

      <h2>Overnight autopilot</h2>
      <Autopilot onLog={(body) => panel.open({ title: 'Autopilot log', body })} show={show} />

      <h2>Farm operations</h2>
      <Ops show={show} />

      <h2>
        Farm-wide limits <span className="h2-note">(farm.conf, every Mac)</span>
      </h2>
      <FarmConf show={show} />

      <h2>Web gateway</h2>
      <Gateway config={config} onSaved={onConfig} show={show} />
    </section>
  );
}

function CheckRow({ c, onDone, show }: { c: Check; onDone: () => Promise<void>; show: Show }) {
  return (
    <div className={`chk ${c.status}`}>
      <div className="g">{MARK[c.status] || '•'}</div>
      <div className="b">
        <div className="l">{c.label}</div>
        <div className="d">{c.detail}</div>
        {c.fix && <div className="fx">{c.fix}</div>}
        {c.action && (
          <div style={{ marginTop: 7 }}>
            <Btn
              label={c.action_label || 'Fix'}
              onClick={async () => {
                try {
                  const msg = await call<string>('run_action', { action: c.action });
                  show(String(msg), 'good');
                  setTimeout(() => void onDone(), 1200);
                } catch (e) {
                  show(errText(e), 'bad');
                }
              }}
            />
          </div>
        )}
      </div>
    </div>
  );
}

/* --- the share settings ------------------------------------------------- */
function ShareSettings({
  config,
  onSaved,
  show,
}: {
  config: ConfigResponse | null;
  onSaved: () => void;
  show: Show;
}) {
  const c = config?.config;
  const [coord, setCoord] = useState('');
  const [shareName, setShareName] = useState('RenderFarm');
  const [sharePath, setSharePath] = useState('');
  const [perf, setPerf] = useState('auto');
  const [disk, setDisk] = useState('15');
  const [repo, setRepo] = useState('');
  const [ltx, setLtx] = useState('');
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    if (!c || loaded) return;
    setCoord(c.coordinator || '');
    setShareName(c.share_name || 'RenderFarm');
    setSharePath(c.share_path || '');
    setPerf(c.perf || 'auto');
    setDisk(String(c.min_free_gb || 15));
    setRepo(c.repo_dir || '');
    setLtx(c.ltx_dir || '');
    setLoaded(true);
  }, [c, loaded]);

  return (
    <form
      autoComplete="off"
      className="cfg card"
      id="cfg"
      onSubmit={async (e) => {
        e.preventDefault();
        try {
          await call('save_config', {
            cfg: {
              coordinator: coord.trim(),
              share_path: sharePath.trim(),
              share_name: shareName.trim() || 'RenderFarm',
              perf,
              min_free_gb: parseInt(disk, 10) || 15,
              ltx_dir: ltx.trim(),
              lora_dir: '',
              repo_dir: repo.trim(),
            },
          });
          show('Saved. The watcher picks up the new folder within 2s.', 'good');
          onSaved();
        } catch (err) {
          show(errText(err), 'bad');
        }
      }}
    >
      <div className="two">
        <div className="f">
          <label htmlFor="f-coord">Coordinator Mac</label>
          <input id="f-coord" onChange={(e) => setCoord(e.currentTarget.value)} placeholder="mac-studio" spellCheck={false} value={coord} />
        </div>
        <div className="f">
          <label htmlFor="f-sharename">Folder name</label>
          <input
            id="f-sharename"
            onChange={(e) => setShareName(e.currentTarget.value)}
            placeholder="RenderFarm"
            spellCheck={false}
            value={shareName}
          />
        </div>
      </div>
      <div className="hint" id="h-url">
        smb://{(coord || '…').trim()}.local/{(shareName || 'RenderFarm').trim()}
      </div>
      <div className="f">
        <label htmlFor="f-share">Farm folder on this Mac</label>
        <input
          id="f-share"
          onChange={(e) => setSharePath(e.currentTarget.value)}
          placeholder={config?.resolved.root || '/Volumes/RenderFarm'}
          spellCheck={false}
          value={sharePath}
        />
      </div>
      <div className="two">
        <div className="f">
          <label htmlFor="f-perf">Speed profile</label>
          <select id="f-perf" onChange={(e) => setPerf(e.currentTarget.value)} value={perf}>
            <option value="auto">auto — size to this Mac&apos;s RAM</option>
            <option value="full">full — dedicated render Mac</option>
            <option value="light">light — someone&apos;s daily driver</option>
          </select>
        </div>
        <div className="f">
          <label htmlFor="f-disk">Min free disk (GB)</label>
          <input id="f-disk" min="1" onChange={(e) => setDisk(e.currentTarget.value)} step="1" type="number" value={disk} />
        </div>
      </div>
      <div className="f">
        <label htmlFor="f-repo">Farm scripts folder</label>
        <input id="f-repo" onChange={(e) => setRepo(e.currentTarget.value)} placeholder="auto-detect" spellCheck={false} value={repo} />
      </div>
      <div className="f">
        <label htmlFor="f-ltx">LTX2-MLX folder</label>
        <input
          id="f-ltx"
          onChange={(e) => setLtx(e.currentTarget.value)}
          placeholder={config?.resolved.ltx_dir || '~/video-gen/LTX2-MLX'}
          spellCheck={false}
          value={ltx}
        />
      </div>
      <div className="bar-actions" style={{ marginTop: 2 }}>
        <button className="btn pri" type="submit">
          Save &amp; re-check
        </button>
        <span className="grow" />
        <Btn
          id="btn-mount"
          label="Connect share"
          onClick={async () => {
            try {
              const msg = await call<string>('run_action', { action: 'mount_share' });
              show(String(msg), 'good');
            } catch (e) {
              show(errText(e), 'bad');
            }
          }}
        />
      </div>
    </form>
  );
}

/* --- autopilot ---------------------------------------------------------- */
function Autopilot({ onLog, show }: { onLog: (body: React.ReactNode) => void; show: Show }) {
  const [a, setA] = useState<AutopilotResponse | null>(null);
  const [on, setOn] = useState(false);
  const [retry, setRetry] = useState('1');
  const [stale, setStale] = useState('20');
  const [streak, setStreak] = useState('5');
  const [loaded, setLoaded] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const r = await call<AutopilotResponse>('get_autopilot');
      setA(r);
      if (!loaded) {
        setOn(r.on);
        setRetry(String(r.policy.retry));
        setStale(String(r.policy.stale_min));
        setStreak(String(r.policy.fail_streak));
        setLoaded(true);
      }
    } catch {
      /* the status line shows the error via `a` staying null */
    }
  }, [loaded]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const status = !a
    ? '—'
    : !a.on
      ? a.supervisor
        ? `Off here — ${a.supervisor} is babysitting the farm.`
        : 'Off. Nothing will be requeued or paused while you sleep.'
      : a.supervisor && a.supervisor !== a.you
        ? `On, but ${a.supervisor} already holds the shift — this Mac stays out of the way.`
        : `On — this Mac is watching the farm.${a.held ? ` ${a.held} job(s) are held.` : ''}`;

  return (
    <form
      autoComplete="off"
      className="cfg card"
      id="autocfg"
      onSubmit={async (e) => {
        e.preventDefault();
        try {
          await call('set_autopilot', {
            on,
            retry: parseInt(retry, 10) || 0,
            stale_min: parseInt(stale, 10) || 20,
            fail_streak: parseInt(streak, 10) || 5,
          });
          show(
            on
              ? 'Autopilot on. It only ever requeues work, and it pauses the farm rather than looping on a fault.'
              : 'Autopilot off for this Mac.',
            'good'
          );
          await refresh();
        } catch (err) {
          show(errText(err), 'bad');
        }
      }}
    >
      <label className="ck">
        <input checked={on} id="a-on" onChange={(e) => setOn(e.currentTarget.checked)} type="checkbox" />{' '}
        <span>This Mac babysits the farm overnight</span>
      </label>
      <div className="gw-url" id="a-status">
        {status}
      </div>
      <div className="two">
        <div className="f">
          <label htmlFor="a-retry">Retry a failure</label>
          <select id="a-retry" onChange={(e) => setRetry(e.currentTarget.value)} value={retry}>
            <option value="0">never</option>
            <option value="1">once</option>
            <option value="2">twice</option>
            <option value="3">3 times</option>
          </select>
        </div>
        <div className="f">
          <label htmlFor="a-stale">Requeue a stalled job after (min)</label>
          <input id="a-stale" max="240" min="5" onChange={(e) => setStale(e.currentTarget.value)} type="number" value={stale} />
        </div>
      </div>
      <div className="f">
        <label htmlFor="a-streak">Pause the whole queue after this many failures in a row</label>
        <input id="a-streak" max="50" min="2" onChange={(e) => setStreak(e.currentTarget.value)} type="number" value={streak} />
      </div>
      <div className="bar-actions" style={{ marginTop: 2 }}>
        <button className="btn pri" type="submit">
          Apply
        </button>
        <span className="grow" />
        <button
          className="btn sm"
          id="a-log"
          onClick={async () => {
            try {
              const r = await call<AutopilotResponse>('get_autopilot');
              onLog(
                <pre>
                  {(r.log || []).join('\n') ||
                    'Nothing yet — autopilot writes here when it does something.'}
                </pre>
              );
            } catch (e) {
              onLog(<pre>{errText(e)}</pre>);
            }
          }}
          type="button"
        >
          What did it do?
        </button>
      </div>
    </form>
  );
}

/* --- farm operations ---------------------------------------------------- */
function Ops({ show }: { show: Show }) {
  const [held, setHeld] = useState(0);

  const refresh = useCallback(async () => {
    try {
      const r = await call<AutopilotResponse>('get_autopilot');
      setHeld(r.held || 0);
    } catch {
      /* nothing to say */
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const op = async (action: string) => {
    try {
      const r = await call<{ message: string }>('farm_action', { action });
      show(r.message, 'good');
      await refresh();
    } catch (e) {
      show(errText(e), 'bad');
    }
  };

  return (
    <div className="card ops">
      <div className="ops-row">
        <div>
          <b>Requeue stalled jobs</b>
          <span>
            A Mac that died mid-render leaves its job in <code>running/</code>. This gives it back to the
            farm.
          </span>
        </div>
        <button className="btn sm" id="op-reap" onClick={() => void op('reap')} type="button">
          Reap
        </button>
      </div>
      <div className="ops-row">
        <div>
          <b>Pause the queue</b>
          <span>Holds every waiting job. Anything already rendering finishes normally.</span>
        </div>
        <button className="btn sm" id="op-pause" onClick={() => void op('pause')} type="button">
          Pause
        </button>
      </div>
      <div className="ops-row">
        <div>
          <b>Resume</b>
          <span id="op-resume-sub">
            {held ? `${held} job(s) are held right now.` : 'Put held jobs back in the queue.'}
          </span>
        </div>
        <button className="btn sm" id="op-resume" onClick={() => void op('resume')} type="button">
          Resume
        </button>
      </div>
    </div>
  );
}

/* --- farm.conf ---------------------------------------------------------- */
function FarmConf({ show }: { show: Show }) {
  const [conf, setConf] = useState<FarmConfResponse | null>(null);
  const [edits, setEdits] = useState<Record<string, string>>({});
  const [error, setError] = useState('');

  const refresh = useCallback(async () => {
    try {
      const r = await call<FarmConfResponse>('get_farm_conf');
      setConf(r);
      setEdits({});
      setError('');
    } catch (e) {
      setError(errText(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const val = (k: ConfKey) => edits[k.key] ?? k.value;

  return (
    <form
      autoComplete="off"
      className="cfg card"
      id="confcfg"
      onSubmit={async (e) => {
        e.preventDefault();
        const keys: Record<string, string> = {};
        for (const k of conf?.keys ?? []) {
          if (edits[k.key] !== undefined && edits[k.key] !== k.value) keys[k.key] = edits[k.key]!;
        }
        if (!Object.keys(keys).length) {
          show('Nothing changed.', 'bad');
          return;
        }
        try {
          const r = await call<{ message: string }>('save_farm_conf', { keys });
          show(r.message, 'good');
          await refresh();
        } catch (err) {
          show(errText(err), 'bad');
        }
      }}
    >
      <div id="conf-fields">
        {(conf?.keys ?? []).map((k) => (
          <div className="f" key={k.key}>
            <label htmlFor={`cf-${k.key}`}>{k.label}</label>
            {k.kind === 'choice' ? (
              <select
                id={`cf-${k.key}`}
                onChange={(e) => {
                  const v = e.currentTarget.value; // read before the updater runs
                  setEdits((s) => ({ ...s, [k.key]: v }));
                }}
                value={val(k)}
              >
                {k.choices.map((o) => (
                  <option key={o} value={o}>
                    {o}
                  </option>
                ))}
              </select>
            ) : (
              <input
                id={`cf-${k.key}`}
                max={k.kind === 'int' ? k.max : undefined}
                min={k.kind === 'int' ? k.min : undefined}
                onChange={(e) => {
                  const v = e.currentTarget.value; // read before the updater runs
                  setEdits((s) => ({ ...s, [k.key]: v }));
                }}
                spellCheck={false}
                type={k.kind === 'int' ? 'number' : 'text'}
                value={val(k)}
              />
            )}
            <div className="hint" style={{ margin: '4px 0 0' }}>
              {k.help}
            </div>
          </div>
        ))}
      </div>
      <div className="bar-actions" style={{ marginTop: 2 }}>
        <button className="btn pri" type="submit">
          Apply to every Mac
        </button>
        <span className="grow" />
        <span className="hint" id="conf-path" style={{ margin: 0 }} title={conf?.path || ''}>
          {error || (conf?.exists ? conf.path : 'farm.conf not found on the share')}
        </span>
      </div>
    </form>
  );
}

/* --- the web gateway ---------------------------------------------------- */
function Gateway({
  config,
  onSaved,
  show,
}: {
  config: ConfigResponse | null;
  onSaved: () => void;
  show: Show;
}) {
  const g = config?.gateway ?? null;
  const [on, setOn] = useState(true);
  const [auto, setAuto] = useState(true);
  const [lan, setLan] = useState(false);
  const [port, setPort] = useState('8787');
  const [loaded, setLoaded] = useState(false);
  const [last, setLast] = useState<typeof g>(null);

  useEffect(() => {
    if (!config || loaded) return;
    setOn(!!config.gateway?.enabled);
    setLan(!!config.gateway?.lan);
    setPort(String(config.gateway?.port ?? 8787));
    setAuto(config.config.web_open_on_launch !== false);
    setLast(config.gateway ?? null);
    setLoaded(true);
  }, [config, loaded]);

  const info = last ?? g;
  const url = info ? (info.lan ? info.lan_url || info.local_url : info.local_url) : '';
  const urlText = !info?.enabled
    ? 'Off — nothing is being served.'
    : info.running
      ? url
      : 'Enabled, but the port couldn’t be bound. Try another port.';

  return (
    <form
      autoComplete="off"
      className="cfg card"
      id="webcfg"
      onSubmit={async (e) => {
        e.preventDefault();
        try {
          const r = await call<{ gateway: typeof g }>('save_config', {
            cfg: {
              web_enabled: on,
              web_lan: lan,
              web_port: parseInt(port, 10) || 8787,
              web_open_on_launch: auto,
            },
          });
          setLast(r.gateway ?? null);
          show(
            lan
              ? 'Applied. Share the link — it carries the key that lets your team in.'
              : 'Applied. The gateway is on this Mac only.',
            'good'
          );
          onSaved();
        } catch (err) {
          show(errText(err), 'bad');
        }
      }}
    >
      <label className="ck">
        <input checked={on} id="w-on" onChange={(e) => setOn(e.currentTarget.checked)} type="checkbox" />{' '}
        <span>Serve this app in a browser</span>
      </label>
      <label className="ck">
        <input checked={auto} id="w-auto" onChange={(e) => setAuto(e.currentTarget.checked)} type="checkbox" />{' '}
        <span>Open the browser view when the app starts</span>
      </label>
      <label className="ck">
        <input checked={lan} id="w-lan" onChange={(e) => setLan(e.currentTarget.checked)} type="checkbox" />{' '}
        <span>Let the team reach it over the office network</span>
      </label>
      <div className="f" style={{ marginTop: 8 }}>
        <label htmlFor="w-port">Port</label>
        <input id="w-port" max="65535" min="1024" onChange={(e) => setPort(e.currentTarget.value)} type="number" value={port} />
      </div>
      <div className="gw-url" id="w-url">
        {urlText}
      </div>
      <div className="warn-note" hidden={!lan} id="w-warn">
        Anyone on this network who has the link can queue renders and run setup steps on this Mac. Only
        turn this on for an office network you trust, and share the link with your team rather than
        posting it anywhere public.
      </div>
      <div className="bar-actions" style={{ marginTop: 10 }}>
        <button className="btn pri" type="submit">
          Apply
        </button>
        <button
          className="btn sm"
          id="w-copy"
          onClick={async () => {
            if (!urlText.startsWith('http')) {
              show('Nothing to copy — turn the gateway on first.', 'bad');
              return;
            }
            try {
              await navigator.clipboard.writeText(urlText);
              show('Link copied.', 'good');
            } catch {
              show(`Couldn’t reach the clipboard — the link is: ${urlText}`, 'bad');
            }
          }}
          type="button"
        >
          Copy link
        </button>
        <button
          className="btn sm"
          id="w-open"
          onClick={() => {
            if (!urlText.startsWith('http')) {
              show('Turn the gateway on first.', 'bad');
              return;
            }
            window.open(urlText, '_blank', 'noopener');
          }}
          type="button"
        >
          Open
        </button>
      </div>
    </form>
  );
}
