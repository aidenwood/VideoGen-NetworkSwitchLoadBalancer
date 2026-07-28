/* Team — who's connected and what their Mac is doing. Three sources merged in
   the backend (presence file, worker .info, live heartbeat); this just tells the
   truth about each row, including the Macs that only look online. */
import { useCallback, useEffect, useState } from 'react';
import { call, errText, secs, WEB } from '../api';
import { Btn, Empty, Tag } from '../ui';
import type { Member, MembersResponse } from '../types';

const STATE_WORD: Record<string, string> = {
  rendering: 'rendering',
  idle: 'idle — waiting for a job',
  paused: 'paused',
  backoff: 'cooling down after a memory kill',
  offline: 'not running the app',
};

const initials = (name: string, host: string) => {
  const src = (name || host || '?').trim();
  const parts = src.split(/[\s_-]+/).filter(Boolean);
  const first = parts[0]?.[0] ?? '?';
  const last = parts.length > 1 ? parts[parts.length - 1]![0] : '';
  return (first + last).toUpperCase();
};

export default function Team({
  active,
  show,
}: {
  active: boolean;
  show: (m: string, k?: 'good' | 'bad') => void;
}) {
  const [data, setData] = useState<MembersResponse | null>(null);
  const [error, setError] = useState('');
  const [name, setName] = useState('');
  const [touched, setTouched] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const r = await call<MembersResponse>('get_members');
      setData(r);
      setError('');
      if (!touched && r.member) setName(r.member);
    } catch (e) {
      setError(errText(e));
    }
  }, [touched]);

  useEffect(() => {
    // Even when this tab is closed the count matters (the planner divides by it),
    // so one fetch happens on mount either way.
    void refresh();
    if (!active) return;
    const t = setInterval(() => {
      if (document.hasFocus()) void refresh();
    }, 5000);
    return () => clearInterval(t);
  }, [active, refresh]);

  const list = data?.members ?? [];

  return (
    <section aria-labelledby="tab-team" className={`view${active ? ' on' : ''}`} id="view-team" role="tabpanel">
      <div className="card whoami">
        <label htmlFor="t-name">Your name on the farm</label>
        <div className="row">
          <input
            id="t-name"
            onChange={(e) => {
              setTouched(true);
              setName(e.currentTarget.value);
            }}
            placeholder="Aiden"
            spellCheck={false}
            value={name}
          />
          <Btn
            id="t-save"
            label="Save"
            onClick={async () => {
              try {
                await call('set_member', { name });
                show('Saved — the rest of the farm sees it within ~10s.', 'good');
                setTouched(false);
                await refresh();
              } catch (e) {
                show(errText(e), 'bad');
              }
            }}
          />
        </div>
        <p className="sub" style={{ margin: '8px 0 0' }}>
          Everyone running the app sees this next to your Mac, so the team knows whose machine is busy.
        </p>
      </div>

      <h2>Macs on the farm</h2>
      <div id="team">
        {error ? (
          <Empty glyph="⚠️" line="Couldn’t read the team" small={error} />
        ) : list.length ? (
          list.map((m) => <MemberRow key={m.host} m={m} />)
        ) : (
          <Empty
            glyph="🖥️"
            line={data?.reachable ? 'Nobody has joined yet' : 'Farm folder not reachable'}
            small={
              data?.reachable
                ? 'Every Mac running this app shows up here with what it’s working on.'
                : 'Mount the share in Checks and this fills in.'
            }
          />
        )}
      </div>
    </section>
  );
}

function MemberRow({ m }: { m: Member }) {
  return (
    <div className={`mem ${m.state || 'idle'}`}>
      <div className="av">{initials(m.member, m.host)}</div>
      <div className="b">
        <div className="n">
          <b>{m.member || m.host}</b>
          {m.is_you && <span className="you">you</span>}
          <span className="hostname">{m.host}</span>
        </div>

        <div className="doing">
          {m.state === 'rendering' && m.job ? (
            <>
              rendering <b>{m.job}</b>
              {` · ${secs(m.elapsed_secs)} in`}
              {m.job_prompt && <div className="jp">{m.job_prompt}</div>}
            </>
          ) : (
            m.detail || STATE_WORD[m.state] || m.state || 'idle'
          )}
        </div>

        <div className="meta">
          {m.model && <Tag>{m.model}</Tag>}
          {m.ram_gb > 0 && <Tag>{m.ram_gb} GB</Tag>}
          {m.role && <Tag>{m.role}</Tag>}
          {m.perf && <Tag>profile {m.perf}</Tag>}
          {m.done_count > 0 && <Tag>{m.done_count} finished</Tag>}
          {/* Two different "not really helping" states, and the difference
              matters: no worker = it isn't rendering; no app = nobody can drive
              it from here. */}
          {m.worker === false && <Tag cls="warm">no worker running</Tag>}
          {m.app === false && <Tag cls="warm">app not running</Tag>}
          {m.swap_mb > 2048 && <Tag cls="warm">swapping {Math.round(m.swap_mb / 1024)} GB</Tag>}
        </div>

        {/* Their gateway, if they're sharing it: one click to drive their Mac's
            setup from here. Only useful in a browser. */}
        {WEB && m.gateway && !m.is_you && (
          <div style={{ marginTop: 8 }}>
            <a className="btn sm" href={m.gateway} rel="noopener" target="_blank">
              Open their board
            </a>
          </div>
        )}
      </div>
    </div>
  );
}
