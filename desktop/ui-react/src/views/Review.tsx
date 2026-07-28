/* Review — the cherry-pick loop. Proof stills first (cheap, seconds each),
   finished clips second. Approving here is what tells the team a shot is done. */
import { useCallback, useEffect, useState } from 'react';
import { call, errText, fileUrl } from '../api';
import { Btn, Empty, Seg, Tag, usePanel } from '../ui';
import ClipTile from '../components/ClipTile';
import { setReview } from '../components/JobCardView';
import type { JobCard, Proof, ProofsResponse } from '../types';

export default function Review({
  active,
  show,
}: {
  active: boolean;
  show: (m: string, k?: 'good' | 'bad') => void;
}) {
  const panel = usePanel();
  const [mode, setMode] = useState<'proofs' | 'clips'>('proofs');
  const [data, setData] = useState<ProofsResponse | null>(null);
  const [error, setError] = useState('');

  const refresh = useCallback(async () => {
    try {
      setData(await call<ProofsResponse>('get_proofs'));
      setError('');
    } catch (e) {
      setError(errText(e));
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void refresh();
    const t = setInterval(() => {
      if (document.hasFocus()) void refresh();
    }, 8000);
    return () => clearInterval(t);
  }, [active, refresh]);

  const review = async (id: string, state: '' | 'approved' | 'retake') => {
    try {
      const r = await setReview(id, state);
      show(r?.message || 'Saved', 'good');
      await refresh();
    } catch (e) {
      show(errText(e), 'bad');
    }
  };

  const renderHero = async (file: string) => {
    try {
      const r = await call<{ message: string }>('job_action', { action: 'render_hero', file });
      show(r?.message || 'Queued', 'good');
      await refresh();
    } catch (e) {
      show(errText(e), 'bad');
    }
  };

  const showStill = (p: Proof) => {
    const src = fileUrl(p.path);
    panel.open({
      title: p.id,
      body: (
        <>
          {src && <img alt={p.id} src={src} style={{ width: '100%', borderRadius: 10 }} />}
          <p className="sub">{p.prompt}</p>
          <div className="acts">
            <Btn
              label={p.review === 'approved' ? '✓ Approved' : 'Approve'}
              onClick={() => review(p.id, p.review === 'approved' ? '' : 'approved')}
              pri={p.review !== 'approved'}
            />
            <Btn
              label={p.review === 'retake' ? '↺ Retake' : 'Needs another'}
              onClick={() => review(p.id, p.review === 'retake' ? '' : 'retake')}
            />
            {!p.rendered && p.done_file && (
              <Btn label="Render hero" onClick={() => renderHero(p.done_file)} pri />
            )}
          </div>
        </>
      ),
    });
  };

  const showClip = (c: JobCard) => {
    const src = c.mp4 ? fileUrl(c.mp4) : null;
    panel.open({
      title: c.id,
      body: (
        <>
          {src && <video autoPlay controls playsInline src={src} />}
          <p className="sub">{c.prompt}</p>
        </>
      ),
    });
  };

  const proofs = data?.proofs ?? [];
  const clips = data?.clips ?? [];
  const approved = clips.filter((c) => c.review === 'approved').length;

  return (
    <section aria-labelledby="tab-review" className={`view${active ? ' on' : ''}`} id="view-review" role="tabpanel">
      <Seg
        active={mode}
        idPrefix="rtab-"
        label="Review mode"
        onPick={(k) => setMode(k as 'proofs' | 'clips')}
        tabs={[
          { key: 'proofs', label: 'Proof stills' },
          { key: 'clips', label: 'Finished clips' },
        ]}
        thumbId="rthumb"
      />

      <div className="review-head">
        <span className="hint" id="review-note" style={{ margin: 0 }}>
          {error ||
            (data?.reachable
              ? `${proofs.length} stills · ${clips.length} clips · ${approved} approved`
              : 'Farm folder not reachable — mount the share in Checks.')}
        </span>
        <span className="grow" />
        <Btn id="rv-refresh" label="Refresh" onClick={refresh} />
      </div>

      <div className="grid" hidden={mode !== 'proofs'} id="rv-proofs">
        {proofs.length ? (
          proofs.map((p) => (
            <ProofTile
              key={`${p.id}-${p.seed}`}
              onRenderHero={() => renderHero(p.done_file)}
              onReview={review}
              onWatch={() => showStill(p)}
              p={p}
            />
          ))
        ) : (
          <Empty
            glyph="🖼"
            line="No proof stills yet"
            small="Queue a job as a proof (or plan an overnight run as proofs) and the stills land here to cherry-pick."
          />
        )}
      </div>

      <div className="grid" hidden={mode !== 'clips'} id="rv-clips">
        {clips.length ? (
          clips.map((c) => (
            <ClipTile card={c} key={c.file} onReview={review} onWatch={showClip} />
          ))
        ) : (
          <Empty
            glyph="🎬"
            line="Nothing finished yet"
            small="Finished renders show up here to approve or send back."
          />
        )}
      </div>
    </section>
  );
}

function ProofTile({
  p,
  onReview,
  onRenderHero,
  onWatch,
}: {
  p: Proof;
  onReview: (id: string, state: '' | 'approved' | 'retake') => Promise<void>;
  onRenderHero: () => Promise<void>;
  onWatch: () => void;
}) {
  const src = fileUrl(p.path);
  return (
    <div className={`tile${p.review ? ` ${p.review}` : ''}`}>
      {src && <img alt={p.id} className="shot" loading="lazy" onClick={onWatch} src={src} />}
      <div className="meta">
        <div className="id">{p.id}</div>
        <div className="pr">{p.prompt}</div>
        <div className="tags">
          <Tag>seed {p.seed}</Tag>
          {p.rendered && <Tag cls="done-badge">hero rendered</Tag>}
        </div>
      </div>
      <div className="acts">
        <Btn
          label={p.review === 'approved' ? '✓ Approved' : 'Approve'}
          onClick={() => onReview(p.id, p.review === 'approved' ? '' : 'approved')}
          pri={p.review !== 'approved'}
        />
        <Btn
          label={p.review === 'retake' ? '↺ Retake' : 'Needs another'}
          onClick={() => onReview(p.id, p.review === 'retake' ? '' : 'retake')}
        />
        {/* A still whose hero render already exists doesn't beg to be rendered
            again — that's how you pay for the same clip twice. */}
        {!p.rendered && p.done_file && <Btn label="Render hero" onClick={onRenderHero} pri />}
      </div>
    </div>
  );
}
