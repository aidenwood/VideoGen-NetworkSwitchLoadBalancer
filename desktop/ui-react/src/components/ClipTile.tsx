/* A finished clip in a grid — the review surface's unit, and the run report's.
   Poster frames come from the gateway's /poster (one ffmpeg frame, cached on the
   share), so every teammate's browser reuses the same jpg. */
import { fileUrl, posterUrl, secs } from '../api';
import { Btn, Tag } from '../ui';
import type { JobCard } from '../types';

export default function ClipTile({
  card: c,
  onWatch,
  onVariants,
  onReview,
  readOnly,
}: {
  card: JobCard;
  onWatch?: (c: JobCard) => void;
  onVariants?: (c: JobCard) => void;
  onReview?: (id: string, state: '' | 'approved' | 'retake') => Promise<void>;
  readOnly?: boolean;
}) {
  const poster = c.mp4 ? posterUrl(c.mp4) : null;
  const playable = c.mp4 ? fileUrl(c.mp4) : null;

  return (
    <div className={`tile${c.review ? ` ${c.review}` : ''}`}>
      {poster && (
        <img
          alt={c.id}
          className="shot"
          loading="lazy"
          onClick={() => onWatch?.(c)}
          onError={(e) => e.currentTarget.remove()}
          src={poster}
        />
      )}
      <div className="meta">
        <div className="id">{c.id}</div>
        <div className="pr">{c.prompt}</div>
        <div className="tags">
          <Tag>{c.aspect || `${c.width}×${c.height}`}</Tag>
          {c.host && <Tag cls="host">{c.host}</Tag>}
          {c.duration_secs > 0 && <Tag>{secs(c.duration_secs)}</Tag>}
        </div>
      </div>
      {!readOnly && onReview && (
        <div className="acts">
          <Btn
            label={c.review === 'approved' ? '✓ Approved' : 'Approve'}
            onClick={() => onReview(c.id, c.review === 'approved' ? '' : 'approved')}
            pri={c.review !== 'approved'}
          />
          <Btn
            label={c.review === 'retake' ? '↺ Retake' : 'Needs another'}
            onClick={() => onReview(c.id, c.review === 'retake' ? '' : 'retake')}
          />
          {playable && onWatch && <Btn label="▶ Watch" onClick={() => onWatch(c)} />}
          {onVariants && <Btn label="Variants…" onClick={() => onVariants(c)} />}
        </div>
      )}
    </div>
  );
}
