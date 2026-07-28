/* One card on the board. Deliberately the same shape in every lane so the eye
   can track a job across the board; only the actions differ. */
import { call, fileUrl, posterUrl, secs, WEB } from '../api';
import { Btn, Tag } from '../ui';
import type { JobCard } from '../types';

export type CardActions = {
  /** Runs a job_action and refreshes the board. */
  act: (args: Record<string, unknown>) => Promise<void>;
  variants: (card: JobCard) => void;
  log: (card: JobCard) => void;
  clip: (card: JobCard) => void;
  review: (id: string, state: '' | 'approved' | 'retake') => Promise<void>;
  selected?: boolean;
  onSelect?: (card: JobCard, on: boolean) => void;
  onDragStart?: (card: JobCard) => void;
  onDragEnd?: () => void;
};

const LANE_CLASS: Record<string, string> = { queued: 'q', running: 'r', done: 'd', failed: 'f' };
const MEMORY_RCS = ['137', '134', '9'];

export default function JobCardView({
  card: c,
  actions,
  readOnly,
}: {
  card: JobCard;
  actions?: CardActions;
  readOnly?: boolean;
}) {
  const cls = LANE_CLASS[c.lane] || '';
  const poster = c.lane === 'done' && c.mp4 ? posterUrl(c.mp4) : null;

  const tags = [
    c.aspect ? `${c.aspect} · ${c.width}×${c.height}` : null,
    c.mode === 'test' ? 'proof still' : null,
    c.priority === 'high' ? 'priority' : null,
    c.kind && c.kind !== 't2v' ? c.kind : null,
  ];

  return (
    <div
      className={`jc ${cls}${c.priority === 'high' ? ' hi' : ''}${actions?.selected ? ' sel' : ''}`}
      data-file={c.file}
      data-lane={c.lane}
      draggable={!readOnly && c.lane === 'queued'}
      onDragEnd={() => actions?.onDragEnd?.()}
      onDragStart={() => actions?.onDragStart?.(c)}
    >
      <div className="jh">
        {c.lane === 'queued' && <span className="pos">#{c.position}</span>}
        <span className="jid">{c.id}</span>
      </div>

      {c.prompt && <div className="jp">{c.prompt}</div>}

      {/* A finished clip is a picture, not a filename. The poster comes off the
          gateway (ffmpeg, cached on the share), so the team reuses one frame. */}
      {poster && (
        <img
          alt=""
          className={`poster${c.height > c.width ? ' tall' : ''}`}
          loading="lazy"
          onClick={() => actions?.clip(c)}
          onError={(e) => e.currentTarget.remove()}
          src={poster}
        />
      )}

      <div className="tags">
        {tags.filter(Boolean).map((t, i) => (
          <Tag cls={t === 'proof still' || t === 'priority' ? 'hi' : undefined} key={i}>
            {t}
          </Tag>
        ))}
        {c.host && <Tag cls="host">{c.host}</Tag>}
        {c.member && <Tag cls="who-tag">by {c.member}</Tag>}
        {c.run && <Tag cls="run-tag">{c.run}</Tag>}
        {c.retry > 0 && <Tag cls="hi">retry {c.retry}</Tag>}
        {c.lane === 'queued' && <Tag>seed {c.seed}</Tag>}
        {c.lane === 'running' && <Tag>{secs(c.age_secs)} in</Tag>}
        {c.lane === 'done' && c.duration_secs > 0 && <Tag>took {secs(c.duration_secs)}</Tag>}
        {c.lane === 'done' && c.mp4_mb > 0 && <Tag>{c.mp4_mb} MB</Tag>}
        {c.lane === 'failed' && c.rc && <Tag cls="rc">exit {c.rc}</Tag>}
        {c.oom_retry > 0 && <Tag cls="rc">OOM retry {c.oom_retry}</Tag>}
        {c.review && (
          <Tag cls={`rev ${c.review}`}>
            {c.review === 'approved' ? 'approved' : 'needs another take'}
          </Tag>
        )}
      </div>

      {c.lane === 'running' && (
        <>
          {/* Width is elapsed-against-estimate and the label says "~": an honest
              guess from this farm's own history beats a fake precise percentage. */}
          <div className="bar">
            <i
              className={c.est_secs ? 'real' : undefined}
              style={
                c.est_secs
                  ? { width: `${Math.min(97, Math.round((c.age_secs / c.est_secs) * 100))}%` }
                  : undefined
              }
            />
          </div>
          {c.est_secs > 0 && (
            <div className="eta">
              {c.eta_secs > 0
                ? `~${secs(c.eta_secs)} left  ·  usually ${secs(c.est_secs)}`
                : `running long — usually ${secs(c.est_secs)}`}
            </div>
          )}
        </>
      )}

      {c.lane === 'queued' && c.est_secs > 0 && (
        <div className="eta">
          {`~${secs(c.est_secs)} to render` +
            (c.eta_secs > 0 ? `  ·  starts in ~${secs(c.eta_secs)}` : '  ·  next up')}
        </div>
      )}

      {!readOnly && actions && (
        <div className="acts">
          {c.lane === 'queued' && (
            <>
              {c.priority === 'high' ? (
                <Btn label="↓ Normal" onClick={() => actions.act({ action: 'demote', file: c.file })} />
              ) : (
                <Btn label="↑ Priority" onClick={() => actions.act({ action: 'promote', file: c.file })} />
              )}
              <Btn label="Variants…" onClick={() => actions.variants(c)} />
              <Btn label="Remove" onClick={() => actions.act({ action: 'cancel', file: c.file })} />
            </>
          )}

          {c.lane === 'running' && (
            <>
              <Btn label="Log" onClick={() => actions.log(c)} />
              <Btn label="Variants…" onClick={() => actions.variants(c)} />
            </>
          )}

          {c.lane === 'done' && (
            <>
              {c.mp4 && (
                <>
                  <Btn
                    label={WEB ? '▶ Watch' : 'Reveal clip'}
                    onClick={() =>
                      WEB
                        ? actions.clip(c)
                        : actions.act({ action: 'reveal', lane: c.lane, file: c.file, path: c.mp4 })
                    }
                    pri
                  />
                  {WEB && fileUrl(c.mp4, true) && (
                    <a className="btn sm" download href={fileUrl(c.mp4, true) as string}>
                      Download
                    </a>
                  )}
                </>
              )}
              {/* Approving is the thing the marketing team actually does here. */}
              <Btn
                label={c.review === 'approved' ? '✓ Approved' : 'Approve'}
                onClick={() => actions.review(c.id, c.review === 'approved' ? '' : 'approved')}
              />
              <Btn
                label={c.review === 'retake' ? '↺ Retake' : 'Needs another'}
                onClick={() => actions.review(c.id, c.review === 'retake' ? '' : 'retake')}
              />
              {c.mode === 'test' && (
                <Btn label="Render hero" onClick={() => actions.act({ action: 'render_hero', file: c.file })} pri />
              )}
              <Btn label="Variants…" onClick={() => actions.variants(c)} />
              <Btn label="Run again" onClick={() => actions.act({ action: 'requeue', lane: 'done', file: c.file })} />
            </>
          )}

          {c.lane === 'failed' && (
            <>
              {c.log && <Btn label="Log" onClick={() => actions.log(c)} />}
              <Btn
                label="Requeue"
                onClick={() => actions.act({ action: 'requeue', lane: 'failed', file: c.file })}
                pri
              />
              {/* The two useful answers to a memory kill. Only offered for exit
                  codes that actually mean "killed". */}
              {MEMORY_RCS.includes(String(c.rc)) && (
                <>
                  <Btn
                    label="Bigger Mac"
                    onClick={() => actions.act({ action: 'bigger_mac', lane: 'failed', file: c.file })}
                  />
                  <Btn
                    label="Smaller"
                    onClick={() => actions.act({ action: 'smaller', lane: 'failed', file: c.file })}
                  />
                </>
              )}
              <Btn label="Variants…" onClick={() => actions.variants(c)} />
            </>
          )}
        </div>
      )}

      {/* Multi-select. Bulk work is the difference between usable and unusable
          once a sweep drops 60 cards on the board. */}
      {!readOnly && actions?.onSelect && (
        <input
          checked={!!actions.selected}
          className="pick"
          onChange={(e) => actions.onSelect?.(c, e.currentTarget.checked)}
          onClick={(e) => e.stopPropagation()}
          title="Select for bulk actions"
          type="checkbox"
        />
      )}

      {!readOnly && c.lane === 'queued' && (
        <span className="grip" title="Drag to change what renders next">
          ⋮⋮
        </span>
      )}
    </div>
  );
}

/** Shared by the board and the review grid: set a clip's review state. */
export async function setReview(id: string, state: '' | 'approved' | 'retake') {
  return call<{ message: string }>('set_review', { id, state });
}
