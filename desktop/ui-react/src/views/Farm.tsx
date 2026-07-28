/* The Farm view — the glance. Queue lane, machines, activity, runs, numbers. */
import { useCallback, useEffect, useState } from 'react';
import { call, secs } from '../api';
import { usePoll } from '../hooks';
import { Btn, Empty, usePanel } from '../ui';
import type { Counts, FarmState, RunReport, RunRow, StatsResponse, JobCard } from '../types';
import JobCardView from '../components/JobCardView';
import ClipTile from '../components/ClipTile';

const VERB: Record<string, string> = {
  sent: 'dispatched',
  received: 'picked up',
  done: 'finished',
  failed: 'failed',
};

/* The queue lane. Ticks, not a bar: this is a video farm, so they read as
   frames. Proportional once the queue outgrows the lane. */
const LANE = 46;
function laneClasses(c: Counts | undefined): string[] {
  const want: string[] = [];
  const total = c ? c.queued + c.running + c.done + c.failed : 0;
  if (c && total > 0) {
    const share = (n: number) => Math.round((n / total) * LANE);
    const d = share(c.done);
    const f = c.failed ? Math.max(1, share(c.failed)) : 0;
    const r = c.running ? Math.max(1, share(c.running)) : 0;
    const q = c.queued ? Math.max(1, share(c.queued)) : 0;
    for (let i = 0; i < d; i++) want.push('d');
    for (let i = 0; i < f; i++) want.push('f');
    for (let i = 0; i < r; i++) want.push('r');
    for (let i = 0; i < q; i++) want.push('q');
  }
  while (want.length > LANE) want.pop();
  while (want.length < LANE) want.push('e');
  return want;
}

export default function Farm({
  active,
  state,
  show,
}: {
  active: boolean;
  state: FarmState | null;
  show: (m: string, k?: 'good' | 'bad') => void;
}) {
  const panel = usePanel();
  const [runs, setRuns] = useState<RunRow[]>([]);
  const stats = usePoll<StatsResponse>('get_stats', 20000, active);

  // Runs belong to this view as much as the board, and this view can be the
  // first one opened — so it fetches them itself rather than depending on the
  // Board having been visited.
  const loadRuns = useCallback(async () => {
    try {
      const r = await call<{ runs: RunRow[] }>('get_runs');
      setRuns(r.runs || []);
    } catch {
      /* the stats block reports a dead backend */
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void loadRuns();
    const t = setInterval(() => {
      if (document.hasFocus()) void loadRuns();
    }, 20000);
    return () => clearInterval(t);
  }, [active, loadRuns]);

  const c = state?.counts;
  const events = state?.events ?? [];
  const workers = state?.workers ?? [];
  const st = stats.data?.stats;

  const openReport = async (run: string) => {
    panel.open({ title: `Run “${run}”`, body: <div className="looking">Adding up the night…</div> });
    try {
      const r = await call<RunReport>('get_run_report', { run });
      panel.setBody(<Report r={r} />);
    } catch (e) {
      panel.setBody(<p className="sub">{String(e instanceof Error ? e.message : e)}</p>);
    }
  };

  return (
    <section
      aria-labelledby="tab-dash"
      className={`view${active ? ' on' : ''}`}
      id="view-dash"
      role="tabpanel"
    >
      <div className="card lane">
        <div aria-hidden="true" className="ticks" id="ticks">
          {laneClasses(c).map((k, i) => (
            <span className={`tk ${k}`} key={i} />
          ))}
        </div>
        <div className="legend">
          <span className="lq">
            <i />
            <b id="c-queued">{c?.queued ?? 0}</b> queued
          </span>
          <span className="lr">
            <i />
            <b id="c-running">{c?.running ?? 0}</b> rendering
          </span>
          <span className="ld">
            <i />
            <b id="c-done">{c?.done ?? 0}</b> done
          </span>
          <span className="lf">
            <i />
            <b id="c-failed">{c?.failed ?? 0}</b> failed
          </span>
        </div>
      </div>

      <div hidden={workers.length === 0} id="mach-wrap">
        <h2>Machines</h2>
        <div className="card mach" id="mach">
          {workers.map((w) => (
            <div className={`m ${w.state || 'idle'}`} key={w.host}>
              <span className="d" />
              <span className="who">{w.host}</span>
              <span className="job">{w.state === 'rendering' ? w.job || 'rendering' : 'idle'}</span>
            </div>
          ))}
        </div>
      </div>

      <div hidden={runs.length === 0} id="runs-wrap">
        <h2>Runs</h2>
        <div id="runs">
          {runs.slice(0, 8).map((r) => (
            <RunRowView key={r.run} onReport={() => void openReport(r.run)} r={r} />
          ))}
        </div>
      </div>

      <h2>Activity</h2>
      <div className="feed" id="feed">
        {events.length ? (
          events
            .slice(-7)
            .reverse()
            .map((ev, i) => (
              <div className={`ev ${ev.kind || ''}`} key={`${ev.kind}${ev.id}${ev.ts}${i}`}>
                <span className="d" />
                <span className="t">
                  {(ev.kind === 'sent' ? '' : ev.host && ev.host !== '?' ? `${ev.host} ` : '') +
                    (VERB[ev.kind] || ev.kind || '') +
                    ' '}
                  <b>{ev.id || '?'}</b>
                </span>
              </div>
            ))
        ) : (
          <Empty
            glyph="🎬"
            line="Nothing has run yet"
            small="Queue a clip on the Board (or with enqueue.sh) and it appears here the moment a Mac claims it."
          />
        )}
      </div>

      <div hidden={!st || !st.clips} id="stats-wrap">
        <h2>This farm&apos;s numbers</h2>
        <div className="card stats" id="stats">
          {st && st.clips > 0 && (
            <>
              <Row k="Clips finished" v={String(st.clips)} />
              <Row k="Last 24 hours" v={`${st.clips_24h} clips · ${secs(st.secs_24h)} of render`} />
              <Row k="Average render" v={secs(st.avg_secs)} />
              {st.over_budget > 0 && (
                <Row k="Renders over their memory budget" v={String(st.over_budget)} warn />
              )}
              {st.per_host.length > 0 && (
                <>
                  <h3>Per Mac</h3>
                  {st.per_host.map((h) => (
                    <Row
                      k={h.host}
                      key={h.host}
                      v={`${h.clips} clips · avg ${secs(h.avg_secs)}${h.clips_24h ? ` · ${h.clips_24h} today` : ''}`}
                    />
                  ))}
                </>
              )}
              {st.by_size.length > 0 && (
                <>
                  <h3>By size</h3>
                  {st.by_size.slice(0, 6).map((z) => (
                    <Row k={z.label} key={z.label} v={`${secs(z.avg_secs)} avg · ${z.clips}`} />
                  ))}
                </>
              )}
            </>
          )}
        </div>
      </div>
      {/* keeps `show` in play for future farm-level actions without a lint hole */}
      <span hidden onClick={() => show('')} />
    </section>
  );
}

const Row = ({ k, v, warn }: { k: string; v: string; warn?: boolean }) => (
  <div className="row">
    <span className="k">{k}</span>
    <span className={`v${warn ? ' warn' : ''}`}>{v}</span>
  </div>
);

function RunRowView({ r, onReport }: { r: RunRow; onReport: () => void }) {
  const total = (r.done || 0) + (r.failed || 0) + (r.queued || 0) + (r.running || 0);
  const pct = (v: number) => `${total ? Math.round((v / total) * 100) : 0}%`;
  const sub = [
    `${r.done || 0} done`,
    r.failed ? `${r.failed} failed` : null,
    r.running ? `${r.running} rendering` : null,
    r.queued ? `${r.queued} waiting` : null,
    r.render_secs ? `${secs(r.render_secs)} of render time` : null,
    r.by ? `by ${r.by}` : null,
  ]
    .filter(Boolean)
    .join(' · ');

  return (
    <div className={`run${r.finished ? ' finished' : ''}`}>
      <div className="b">
        <div className="n">
          <b>{r.run}</b>
          <span>{r.finished ? 'finished' : `${(r.done || 0) + (r.failed || 0)}/${total}`}</span>
        </div>
        <div className="prog">
          <i className="d" style={{ width: pct(r.done || 0) }} />
          <i className="f" style={{ width: pct(r.failed || 0) }} />
          <i className="r" style={{ width: pct(r.running || 0) }} />
        </div>
        <p className="sub">{sub}</p>
      </div>
      <Btn label="Report" onClick={onReport} />
    </div>
  );
}

/* The morning report. Deliberately a panel, not a page: you read it, act on the
   failures, and get on with the day. */
function Report({ r }: { r: RunReport }) {
  const c = r.counts;
  const head = [
    `${c.done} done`,
    `${c.failed} failed`,
    c.queued + c.running ? `${c.queued + c.running} still to go` : 'all finished',
    r.render_secs ? `${secs(r.render_secs)} of render time` : null,
  ]
    .filter(Boolean)
    .join('  ·  ');

  return (
    <>
      <div className="plan-sum">{head}</div>
      {(c.approved > 0 || c.retake > 0) && (
        <p className="sub">
          {c.approved} approved, {c.retake} marked for another take.
        </p>
      )}
      {r.per_host.length > 0 && (
        <>
          <h2>Who rendered what</h2>
          <div className="stats card">
            {r.per_host.map((x) => (
              <div className="row" key={x.host}>
                <span className="k">{x.host}</span>
                <span className="v">{x.clips} clips</span>
              </div>
            ))}
          </div>
        </>
      )}
      {r.failed.length > 0 && (
        <>
          <h2>Failed — worth a look</h2>
          {r.failed.map((f: JobCard) => (
            <JobCardView card={f} key={f.file} readOnly />
          ))}
        </>
      )}
      {r.done.length > 0 && (
        <>
          <h2>Finished</h2>
          <div className="grid">
            {r.done.slice(0, 24).map((cl) => (
              <ClipTile card={cl} key={cl.file} readOnly />
            ))}
          </div>
        </>
      )}
    </>
  );
}
