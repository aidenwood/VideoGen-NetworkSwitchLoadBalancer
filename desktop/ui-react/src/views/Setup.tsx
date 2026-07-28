/* Setup — the guided wizard. Recomputed from the real world on every call, never
   from a stored step number, so quitting halfway or doing a step by hand in
   Finder both just work. */
import { useEffect, useState } from 'react';
import { call, errText } from '../api';
import { Btn, InlineError, Looking } from '../ui';
import type { SetupResponse, SetupStep } from '../types';

export default function Setup({
  active,
  setup,
  onChanged,
  onFinish,
  show,
}: {
  active: boolean;
  setup: SetupResponse | null;
  onChanged: () => void;
  onFinish: () => void;
  show: (m: string, k?: 'good' | 'bad') => void;
}) {
  const hasRole = !!setup?.role;
  const steps = setup?.steps ?? [];
  const doneN = steps.filter((s) => s.done).length;
  const now = steps.findIndex((s) => !s.done);

  const setRole = async (role: string) => {
    try {
      await call('set_role', { role });
      onChanged();
    } catch (e) {
      show(errText(e), 'bad');
    }
  };

  return (
    <section aria-labelledby="tab-wiz" className={`view${active ? ' on' : ''}`} id="view-wiz" role="tabpanel">
      <div hidden={hasRole} id="wiz-role">
        <p className="lead">What is this Mac&apos;s job?</p>
        <p className="sub">Asked once. Everything after it is done for you where macOS allows.</p>
        <div className="roles">
          <button className="role" id="role-coord" onClick={() => void setRole('coordinator')} type="button">
            <span className="ic">🗄️</span>
            <span>
              <b>It holds the shared folder</b>
              <span className="why">
                The coordinator. One Mac in the office stores the job queue everyone else reads. It can
                render too.
              </span>
            </span>
          </button>
          <button className="role" id="role-worker" onClick={() => void setRole('worker')} type="button">
            <span className="ic">🎬</span>
            <span>
              <b>It just renders</b>
              <span className="why">
                A worker. Connects to the coordinator&apos;s folder and chews through jobs. Pick this for
                every other Mac.
              </span>
            </span>
          </button>
        </div>
      </div>

      <div hidden={!hasRole} id="wiz-steps">
        <p className="lead" id="w-title">
          {setup?.all_done ? 'This Mac is ready' : `Setting up ${setup?.host ?? ''}`}
        </p>
        <p className="sub" id="w-sub">
          {setup?.all_done
            ? setup.role === 'coordinator'
              ? `Hosting the queue at ${setup.root}. Other Macs can point at it now.`
              : `Connected to ${setup?.root}. Open Farm to watch jobs come through.`
            : (steps[now]?.title ?? '')}
        </p>
        <div className="rail">
          <div className="bar">
            <i id="w-prog" style={{ width: `${Math.round((doneN / Math.max(steps.length, 1)) * 100)}%` }} />
          </div>
          <span className="n" id="w-count">
            {doneN}/{steps.length}
          </span>
        </div>
        <div className="steps" id="w-list">
          {steps.map((s, i) => (
            <StepRow
              i={i}
              isNow={i === now}
              key={s.id}
              onChanged={onChanged}
              s={s}
              show={show}
            />
          ))}
        </div>
        <div className="bar-actions">
          <Btn id="w-recheck" label="Re-check" onClick={async () => onChanged()} />
          <span className="grow" />
          <button className="btn link" id="w-role-again" onClick={() => void setRole('')} type="button">
            Change role
          </button>
          <button
            className="btn pri"
            id="w-finish"
            onClick={async () => {
              try {
                await call('finish_wizard');
              } catch {
                /* the wizard still gets out of the way */
              }
              onFinish();
            }}
            type="button"
          >
            {setup?.all_done ? 'Open Farm' : 'Skip for now'}
          </button>
        </div>
      </div>
    </section>
  );
}

function StepRow({
  s,
  i,
  isNow,
  onChanged,
  show,
}: {
  s: SetupStep;
  i: number;
  isNow: boolean;
  onChanged: () => void;
  show: (m: string, k?: 'good' | 'bad') => void;
}) {
  const [error, setError] = useState<unknown>(null);

  return (
    <div className={`step ${s.done ? 'done' : isNow ? 'now' : 'todo'}`}>
      <div className="n">{s.done ? '✓' : String(i + 1)}</div>
      <div className="body">
        <div className="h">{s.title}</div>
        {/* Only the current step explains itself. Finished ones collapse to a
            line — instructions you've already followed are just noise. */}
        {isNow && !s.done && <div className="p">{s.body}</div>}
        <div className="s">{s.detail}</div>

        {isNow && !s.done && s.id === 'pick' && <HostPicker onPicked={onChanged} show={show} />}

        {isNow && !s.done && s.action && (
          <div className="acts">
            <Btn
              holdOnSuccess
              label={s.action_label}
              onClick={async () => {
                setError(null);
                try {
                  const msg = await call<string>('run_action', { action: s.action });
                  show(String(msg), 'good');
                  setTimeout(onChanged, s.manual ? 400 : 1400);
                } catch (e) {
                  // Inline, not a toast: this is the one thing the user has to read.
                  setError(e);
                  // Rethrow so the button re-enables — a failed action must be
                  // retryable, and holdOnSuccess only applies to success.
                  throw e;
                }
              }}
              pri
            />
            {s.manual && <Btn label="I've done it" onClick={async () => onChanged()} />}
            {(s.id === 'toolchain' || s.id === 'models' || s.id === 'stage') && (
              <Btn
                label="Choose folder…"
                onClick={async () => {
                  try {
                    const msg = await call<string>('pick_repo');
                    show(String(msg), 'good');
                    onChanged();
                  } catch (e) {
                    show(errText(e), 'bad');
                  }
                }}
              />
            )}
          </div>
        )}

        {error !== null && <InlineError e={error} />}
      </div>
    </div>
  );
}

/* Workers pick their coordinator from a Bonjour browse — nobody should need to
   know a hostname to join a farm. */
function HostPicker({
  onPicked,
  show,
}: {
  onPicked: () => void;
  show: (m: string, k?: 'good' | 'bad') => void;
}) {
  const [hosts, setHosts] = useState<string[] | null>(null);

  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        const r = await call<string[]>('discover_coordinators');
        if (alive) setHosts(r || []);
      } catch {
        if (alive) setHosts([]);
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

  if (hosts === null) return <Looking text="Looking for Macs sharing a folder…" />;
  if (!hosts.length) {
    return (
      <div className="looking">
        No Macs found. Check the coordinator has File Sharing on and is plugged into the same switch,
        then Re-check.
      </div>
    );
  }
  return (
    <div className="hosts">
      {hosts.map((h) => (
        <button
          className="host-btn"
          key={h}
          onClick={async () => {
            try {
              await call('set_coordinator', { name: h });
              show(`Coordinator set to ${h}`, 'good');
              onPicked();
            } catch (e) {
              show(errText(e), 'bad');
            }
          }}
          type="button"
        >
          <span>🖥️</span>
          <span>{h}</span>
          <span className="m">sharing</span>
        </button>
      ))}
    </div>
  );
}
