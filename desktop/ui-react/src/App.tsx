/* The shell: header, tabs, the six views, and the two things that make both
   surfaces feel like one app — the 2s heartbeat (which carries `rev`, so a
   setting changed anywhere reloads every open view) and the tray bridge
   (window.__openTab, called by the menubar menu items). */
import { useCallback, useEffect, useRef, useState } from 'react';
import { WEB, KEY, SERVED_OVER_HTTP, call, setGateway } from './api';
import { useRev, useToast } from './hooks';
import { PanelHost, Seg } from './ui';
import type { ConfigResponse, FarmState, SetupResponse } from './types';
import Farm from './views/Farm';
import Board from './views/Board';
import Review from './views/Review';
import Team from './views/Team';
import Checks from './views/Checks';
import Setup from './views/Setup';

export const TABS = ['wiz', 'dash', 'board', 'review', 'team', 'checks'] as const;
export type Tab = (typeof TABS)[number];

const TAB_LABELS: { key: Tab; label: string }[] = [
  { key: 'wiz', label: 'Setup' },
  { key: 'dash', label: 'Farm' },
  { key: 'board', label: 'Board' },
  { key: 'review', label: 'Review' },
  { key: 'team', label: 'Team' },
  { key: 'checks', label: 'Checks' },
];

export default function App() {
  const [tab, setTab] = useState<Tab>('dash');
  const [state, setState] = useState<FarmState | null>(null);
  const [config, setConfig] = useState<ConfigResponse | null>(null);
  const [setup, setSetup] = useState<SetupResponse | null>(null);
  const [booted, setBooted] = useState(false);
  const { toast, show } = useToast();
  const [notifyOn, setNotifyOn] = useState(false);
  const seenEvents = useRef(new Set<string>());

  /* --- config, reloaded whenever anything changes it ---------------------- */
  const loadConfig = useCallback(async () => {
    try {
      const r = await call<ConfigResponse>('get_config');
      setConfig(r);
      setGateway(r.gateway ?? null);
    } catch {
      /* the Checks banner already reports a dead backend */
    }
  }, []);

  const loadSetup = useCallback(async () => {
    try {
      setSetup(await call<SetupResponse>('setup_steps'));
    } catch {
      /* the wizard shows its own error */
    }
  }, []);

  const onRev = useRev(() => {
    void loadConfig();
    void loadSetup();
  });

  /* --- the shared 2s heartbeat ------------------------------------------- */
  useEffect(() => {
    let alive = true;
    const tick = async () => {
      try {
        const s = await call<FarmState>('get_state');
        if (!alive) return;
        setState(s);
        onRev(s.rev);
        notifyEvents(s);
      } catch {
        if (alive) setState(null);
      }
    };
    void tick();
    const t = setInterval(tick, 2000);
    return () => {
      alive = false;
      clearInterval(t);
    };
    // notifyEvents is stable enough (reads refs); rev handler is memoised
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [onRev, notifyOn]);

  /* --- browser notifications -------------------------------------------- */
  const notifyEvents = (s: FarmState) => {
    if (!notifyOn || !('Notification' in window) || Notification.permission !== 'granted') return;
    for (const ev of s.events || []) {
      const key = `${ev.kind}:${ev.id}:${ev.ts || ''}`;
      if (seenEvents.current.has(key)) continue;
      seenEvents.current.add(key);
      if (ev.kind !== 'done' && ev.kind !== 'failed') continue;
      const title = ev.kind === 'done' ? '✅ Render done' : '❌ Render failed';
      const body = (ev.host && ev.host !== '?' ? `${ev.host} · ` : '') + ev.id;
      try {
        new Notification(title, { body, tag: key, icon: '/icon.png' });
      } catch {
        /* the browser can refuse; the toast path still works */
      }
    }
  };

  const toggleBell = async () => {
    if (!('Notification' in window)) return;
    if (notifyOn) {
      setNotifyOn(false);
      try {
        localStorage.setItem('farm.notify', '0');
      } catch { /* private mode */ }
      return;
    }
    const perm =
      Notification.permission === 'granted' ? 'granted' : await Notification.requestPermission();
    if (perm !== 'granted') {
      show('Your browser blocked notifications for this page.', 'bad');
      return;
    }
    setNotifyOn(true);
    try {
      localStorage.setItem('farm.notify', '1');
    } catch { /* private mode */ }
    show('You’ll get a notification as each render lands.', 'good');
  };

  /* --- boot ------------------------------------------------------------- */
  useEffect(() => {
    if (WEB) document.body.classList.add('web');
    try {
      setNotifyOn(localStorage.getItem('farm.notify') === '1' && Notification.permission === 'granted');
    } catch { /* no storage, no bell */ }

    // Installable on a phone — only when a gateway is genuinely serving this
    // page, or the links 404 (inside the app, and in the test harness).
    if (WEB && SERVED_OVER_HTTP) {
      const link = (rel: string, href: string) => {
        const l = document.createElement('link');
        l.rel = rel;
        l.href = href;
        document.head.append(l);
      };
      link('manifest', '/manifest.json' + (KEY ? `?k=${encodeURIComponent(KEY)}` : ''));
      link('apple-touch-icon', '/icon.png');
    }

    void loadConfig();
    void (async () => {
      try {
        const r = await call<SetupResponse>('setup_steps');
        setSetup(r);
        // In a browser the board is what you came for; the popover keeps its
        // glance-first behaviour. An unconfigured Mac still opens on Setup.
        setTab(!r.wizard_done || !r.all_done ? 'wiz' : WEB ? 'board' : 'dash');
      } catch {
        setTab('wiz');
      } finally {
        setBooted(true);
      }
    })();
  }, [loadConfig]);

  /* --- the tray bridge -------------------------------------------------- */
  useEffect(() => {
    window.__openTab = (which: string) => {
      if ((TABS as readonly string[]).includes(which)) setTab(which as Tab);
    };
    return () => {
      delete window.__openTab;
    };
  }, []);

  /* --- keyboard: 1–6 switch views (board adds its own) ------------------ */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = document.activeElement;
      const typing =
        el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT');
      if (typing || e.metaKey || e.ctrlKey || e.altKey) return;
      const n = parseInt(e.key, 10);
      if (n >= 1 && n <= TABS.length) setTab(TABS[n - 1] as Tab);
    };
    addEventListener('keydown', onKey);
    return () => removeEventListener('keydown', onKey);
  }, []);

  const counts = state?.counts;
  const pip = !state ? 'stop' : counts && counts.running > 0 ? 'busy' : state.root ? 'live' : 'stop';
  const stepsLeft = setup?.steps
    ? setup.steps.length - setup.steps.filter((s) => s.done).length
    : 0;
  const host = setup?.host || state?.surface_host || '—';

  return (
    <PanelHost>
      <div className="top">
        <span className={`pip ${pip}`} id="pip" />
        <span className="name">LTX Mac Farm</span>
        <button
          className={`bell${notifyOn ? ' on' : ''}`}
          hidden={!WEB || !('Notification' in window)}
          id="bell"
          onClick={toggleBell}
          title={notifyOn ? 'Telling you when renders finish' : 'Tell me when my renders finish'}
          type="button"
        >
          🔔
        </button>
        <span className="host" id="host">
          {host}
        </span>
      </div>

      <Seg
        active={tab}
        extra={{
          wiz:
            setup && setup.role && !setup.all_done && stepsLeft > 0 ? (
              <span className="badge">{stepsLeft}</span>
            ) : null,
        }}
        idPrefix="tab-"
        label="Views"
        onPick={(k) => setTab(k as Tab)}
        tabs={TAB_LABELS}
        thumbId="thumb"
      />

      <Setup
        active={tab === 'wiz'}
        onChanged={() => {
          void loadSetup();
          void loadConfig();
        }}
        onFinish={() => setTab('dash')}
        setup={setup}
        show={show}
      />
      <Farm active={tab === 'dash'} show={show} state={state} />
      <Board active={tab === 'board'} config={config} onConfig={loadConfig} show={show} />
      <Review active={tab === 'review'} show={show} />
      <Team active={tab === 'team'} show={show} />
      <Checks active={tab === 'checks'} config={config} onConfig={loadConfig} show={show} />

      <div aria-live="polite" className={`toast${toast.visible ? ' show' : ''}${toast.kind ? ` ${toast.kind}` : ''}`} id="toast" role="status">
        {toast.msg}
      </div>
      {!booted && null}
    </PanelHost>
  );
}
