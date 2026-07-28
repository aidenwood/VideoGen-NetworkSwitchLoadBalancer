/* The Board — the pipeline as four lanes, plus everything that puts work into it.
   Search/filters/bulk exist because a 200-clip sweep makes plain lanes
   unreadable; drag-to-reorder exists because claim order IS filename order on
   the share, so dragging is a real operation, not a cosmetic sort. */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { call, errText, fileUrl, secs, uploadUrl } from '../api';
import { Btn, ColEmpty, Looking, usePanel } from '../ui';
import JobCardView, { setReview } from '../components/JobCardView';
import type {
  Assets,
  BoardResponse,
  ConfigResponse,
  JobCard,
  JobLog,
  Lane,
  NewJob,
  Preset,
  Variant,
} from '../types';

type Show = (m: string, k?: 'good' | 'bad') => void;

const LANES: { key: Lane; empty: string }[] = [
  { key: 'queued', empty: 'Nothing waiting. Queue a clip above.' },
  { key: 'running', empty: 'No Mac is rendering right now.' },
  { key: 'done', empty: 'Finished renders land here.' },
  { key: 'failed', empty: 'Nothing has failed. Good.' },
];

const SIZES = [
  { v: '1080x1920', l: 'Vertical 9:16 — Reels, TikTok' },
  { v: '1080x1080', l: 'Square 1:1 — feed' },
  { v: '1920x1080', l: 'Landscape 16:9 — YouTube, hero' },
  { v: '1080x1350', l: 'Portrait 4:5 — IG feed' },
];

const GROUP_TITLE: Record<string, string> = {
  size: 'Other delivery sizes',
  prompt: 'Same shot, edited prompt',
  seed: 'More takes',
  quality: 'Cheaper check first',
};

export default function Board({
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
  const [data, setData] = useState<BoardResponse | null>(null);
  const [err, setErr] = useState('');
  const [q, setQ] = useState('');
  const [fSize, setFSize] = useState('');
  const [fHost, setFHost] = useState('');
  const [fRun, setFRun] = useState('');
  const [fReview, setFReview] = useState('');
  const [picked, setPicked] = useState<Map<string, Lane>>(new Map());
  const [assets, setAssets] = useState<Assets>({ images: [], loras: [] });
  const dragFile = useRef<string | null>(null);
  const searchRef = useRef<HTMLInputElement | null>(null);
  const busy = useRef(false);

  const refresh = useCallback(async () => {
    if (busy.current) return;
    busy.current = true;
    try {
      const r = await call<BoardResponse>('get_board');
      setData(r);
      setErr('');
    } catch (e) {
      setErr(errText(e));
    } finally {
      busy.current = false;
    }
  }, []);

  const loadAssets = useCallback(async () => {
    try {
      const got = await call<Assets>('list_assets');
      setAssets({ images: got?.images ?? [], loras: got?.loras ?? [] });
    } catch {
      setAssets({ images: [], loras: [] });
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void refresh();
    void loadAssets();
    const t = setInterval(() => {
      if (document.hasFocus()) void refresh();
    }, 3000);
    return () => clearInterval(t);
  }, [active, refresh, loadAssets]);

  const board = data?.board;

  /* --- filtering: on data we already have. Re-querying the share to search
         would be slower and no more accurate. ------------------------------ */
  const filterActive = !!(q || fSize || fHost || fRun || fReview);
  const matches = useCallback(
    (c: JobCard) => {
      if (fSize && `${c.width}x${c.height}` !== fSize) return false;
      if (fHost && c.host !== fHost) return false;
      if (fRun && c.run !== fRun) return false;
      if (fReview === 'none' && c.review) return false;
      if (fReview && fReview !== 'none' && c.review !== fReview) return false;
      if (q) {
        const hay = `${c.id} ${c.prompt} ${c.member} ${c.run} ${c.host}`.toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    },
    [q, fSize, fHost, fRun, fReview]
  );

  const all = useMemo(
    () => (board ? [...board.queued, ...board.running, ...board.done, ...board.failed] : []),
    [board]
  );
  const uniq = (f: (c: JobCard) => string) => [...new Set(all.map(f).filter(Boolean))].sort();

  // a job that got claimed or finished can't stay selected
  useEffect(() => {
    if (!board) return;
    const live = new Set(all.map((c) => c.file));
    setPicked((p) => {
      const next = new Map([...p].filter(([f]) => live.has(f)));
      return next.size === p.size ? p : next;
    });
  }, [board, all]);

  /* --- actions ----------------------------------------------------------- */
  const act = useCallback(
    async (args: Record<string, unknown>) => {
      try {
        const r = await call<{ message: string }>('job_action', args);
        show(r?.message || 'Done', 'good');
        await refresh();
      } catch (e) {
        show(errText(e), 'bad');
      }
    },
    [refresh, show]
  );

  const review = useCallback(
    async (id: string, state: '' | 'approved' | 'retake') => {
      try {
        const r = await setReview(id, state);
        show(r?.message || 'Saved', 'good');
        await refresh();
      } catch (e) {
        show(errText(e), 'bad');
      }
    },
    [refresh, show]
  );

  /* --- panels ----------------------------------------------------------- */
  const showClip = (c: JobCard) => {
    const src = c.mp4 ? fileUrl(c.mp4) : null;
    panel.open({
      title: c.id,
      body: (
        <>
          {src && <video autoPlay controls playsInline src={src} />}
          <p className="sub">{c.prompt}</p>
          <div className="tags">
            <span className="tg">
              {c.width}×{c.height}
            </span>
            <span className="tg">seed {c.seed}</span>
            <span className="tg host">{c.host || '?'}</span>
            {c.duration_secs > 0 && <span className="tg">took {secs(c.duration_secs)}</span>}
            {c.peak_mem_gb > 0 && <span className="tg">peak {c.peak_mem_gb} GB</span>}
          </div>
        </>
      ),
    });
  };

  const showLog = (c: JobCard) => {
    panel.open({ title: `${c.id} — log`, body: <LogBody card={c} /> });
  };

  const showVariants = (c: JobCard) => {
    panel.open({
      title: `Variants of ${c.id}`,
      body: <VariantsBody card={c} onQueued={refresh} panelClose={panel.close} show={show} />,
    });
  };

  /* --- drag to reorder --------------------------------------------------- */
  const onDragOver = (e: React.DragEvent) => {
    if (!dragFile.current) return;
    e.preventDefault();
    const col = e.currentTarget as HTMLElement;
    const moving = col.querySelector<HTMLElement>(`.jc[data-file="${cssEscape(dragFile.current)}"]`);
    const over = (e.target as HTMLElement).closest<HTMLElement>('.jc');
    if (!moving || !over || over === moving) return;
    const box = over.getBoundingClientRect();
    const after = e.clientY > box.top + box.height / 2;
    over.parentNode?.insertBefore(moving, after ? over.nextSibling : over);
  };

  const onDragEnd = async () => {
    if (!dragFile.current) return;
    dragFile.current = null;
    const col = document.getElementById('col-queued');
    const order = [...(col?.querySelectorAll<HTMLElement>('.jc') ?? [])].map((el) => el.dataset.file!);
    await act({ action: 'reorder', order });
  };

  /* --- keyboard --------------------------------------------------------- */
  useEffect(() => {
    if (!active) return;
    const onKey = (e: KeyboardEvent) => {
      const el = document.activeElement;
      const typing =
        el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT');
      if (typing || e.metaKey || e.ctrlKey || e.altKey) return;
      if (/^[1-6]$/.test(e.key)) return; // App handles view switching
      if (e.key === '/') {
        e.preventDefault();
        searchRef.current?.focus();
        return;
      }
      if (e.key === 'a') {
        e.preventDefault();
        const next = new Map(picked);
        (board?.queued ?? []).filter(matches).forEach((c) => next.set(c.file, 'queued'));
        setPicked(next);
        return;
      }
      if (!picked.size) return;
      const files = [...picked.keys()];
      const lanes = new Set([...picked.values()]);
      const run = async (args: Record<string, unknown>) => {
        await act(args);
        setPicked(new Map());
      };
      if (e.key === 'p' && lanes.size === 1 && lanes.has('queued')) void run({ action: 'promote', files });
      if (e.key === 'x' && lanes.size === 1 && lanes.has('queued')) void run({ action: 'cancel', files });
      if (e.key === 'r' && lanes.size === 1 && lanes.has('failed'))
        void run({ action: 'requeue', lane: 'failed', files });
    };
    addEventListener('keydown', onKey);
    return () => removeEventListener('keydown', onKey);
  }, [active, board, matches, picked, act]);

  const clearFilters = () => {
    setQ('');
    setFSize('');
    setFHost('');
    setFRun('');
    setFReview('');
  };

  const held = data?.held ?? 0;
  const lanes = new Set([...picked.values()]);
  const files = [...picked.keys()];

  return (
    <section aria-labelledby="tab-board" className={`view${active ? ' on' : ''}`} id="view-board" role="tabpanel">
      <Composer
        assets={assets}
        onAssets={loadAssets}
        onQueued={refresh}
        presets={config?.presets ?? []}
        reachable={!!board?.reachable}
        reloadConfig={onConfig}
        show={show}
      />

      <Planner onQueued={refresh} show={show} />

      <div className="held-banner" hidden={held === 0} id="held-banner">
        <span className="g">⏸</span>
        <span className="tx">
          <b id="held-title">Queue paused</b>
          <span id="held-sub">
            {held} job(s) held. Anything already rendering finishes normally.
          </span>
        </span>
        <Btn
          id="held-resume"
          label="Resume"
          onClick={async () => {
            try {
              const r = await call<{ message: string }>('farm_action', { action: 'resume' });
              show(r.message, 'good');
              await refresh();
            } catch (e) {
              show(errText(e), 'bad');
            }
          }}
        />
      </div>

      <div className="board-head">
        <button className="btn sm" id="b-refresh" onClick={() => void refresh()} type="button">
          Refresh
        </button>
        <input
          className="search"
          id="b-search"
          onChange={(e) => setQ(e.currentTarget.value.trim().toLowerCase())}
          placeholder="Search prompts, names, people…  ( / )"
          ref={searchRef}
          spellCheck={false}
          value={q}
        />
        <span className="hint" id="board-note" style={{ margin: 0 }} title={board?.root || ''}>
          {err || (board?.reachable ? board.root : 'Farm folder not reachable — mount the share in Checks.')}
        </span>
      </div>

      <div className="filters" id="filters">
        <select id="f-lane-size" onChange={(e) => setFSize(e.currentTarget.value)} value={fSize}>
          <option value="">any size</option>
          {uniq((c) => `${c.width}x${c.height}`).map((v) => (
            <option key={v} value={v}>
              {v}
            </option>
          ))}
        </select>
        <select id="f-lane-host" onChange={(e) => setFHost(e.currentTarget.value)} value={fHost}>
          <option value="">any Mac</option>
          {uniq((c) => c.host).map((v) => (
            <option key={v} value={v}>
              {v}
            </option>
          ))}
        </select>
        <select id="f-lane-run" onChange={(e) => setFRun(e.currentTarget.value)} value={fRun}>
          <option value="">any run</option>
          {uniq((c) => c.run).map((v) => (
            <option key={v} value={v}>
              {v}
            </option>
          ))}
        </select>
        <select id="f-lane-review" onChange={(e) => setFReview(e.currentTarget.value)} value={fReview}>
          <option value="">any review</option>
          <option value="approved">approved</option>
          <option value="retake">needs another take</option>
          <option value="none">not reviewed</option>
        </select>
        <button className="btn sm" hidden={!filterActive} id="f-clear" onClick={clearFilters} type="button">
          Clear
        </button>
      </div>

      <div className="bulk" hidden={picked.size === 0} id="bulk">
        <b id="bulk-n">{picked.size} selected</b>
        <span className="grow" />
        <div className="bulk-acts" id="bulk-acts">
          {/* Only what's legal for everything selected — a bulk button that
              half-works is worse than one that isn't there. */}
          {lanes.size === 1 && lanes.has('queued') && (
            <>
              <Btn label="↑ Priority" onClick={async () => { await act({ action: 'promote', files }); setPicked(new Map()); }} />
              <Btn label="↓ Normal" onClick={async () => { await act({ action: 'demote', files }); setPicked(new Map()); }} />
              <Btn label="Remove" onClick={async () => { await act({ action: 'cancel', files }); setPicked(new Map()); }} />
            </>
          )}
          {lanes.size === 1 && lanes.has('failed') && (
            <Btn
              label="Requeue all"
              onClick={async () => { await act({ action: 'requeue', lane: 'failed', files }); setPicked(new Map()); }}
              pri
            />
          )}
          {lanes.size === 1 && lanes.has('done') && (
            <>
              <Btn
                label="Approve all"
                onClick={async () => {
                  const ids = (board?.done ?? []).filter((c) => picked.has(c.file)).map((c) => c.id);
                  let n = 0;
                  for (const id of ids) {
                    try {
                      await setReview(id, 'approved');
                      n++;
                    } catch { /* keep going; the toast reports the tally */ }
                  }
                  show(`${n} clip(s) marked`, 'good');
                  setPicked(new Map());
                  await refresh();
                }}
                pri
              />
              <Btn
                label="Needs another"
                onClick={async () => {
                  const ids = (board?.done ?? []).filter((c) => picked.has(c.file)).map((c) => c.id);
                  let n = 0;
                  for (const id of ids) {
                    try {
                      await setReview(id, 'retake');
                      n++;
                    } catch { /* as above */ }
                  }
                  show(`${n} clip(s) marked`, 'good');
                  setPicked(new Map());
                  await refresh();
                }}
              />
              <Btn label="Run again" onClick={async () => { await act({ action: 'requeue', lane: 'done', files }); setPicked(new Map()); }} />
            </>
          )}
          {lanes.size > 1 && (
            <span className="hint" style={{ margin: 0 }}>
              mixed lanes — pick one lane at a time
            </span>
          )}
        </div>
        <button className="btn link" id="bulk-clear" onClick={() => setPicked(new Map())} type="button">
          Clear
        </button>
      </div>

      <div className="board" id="board">
        {LANES.map((l) => {
          const cards = board?.[l.key] ?? [];
          const shown = cards.filter(matches);
          return (
            <div className="lane-col" data-lane={l.key} key={l.key}>
              <h2>
                {l.key === 'queued'
                  ? 'Queued'
                  : l.key === 'running'
                    ? 'Rendering'
                    : l.key === 'done'
                      ? 'Done'
                      : 'Failed'}{' '}
                <span className="ct" id={`ct-${l.key}`}>
                  {filterActive ? `${shown.length}/${cards.length}` : cards.length}
                </span>
              </h2>
              <div
                className="col-body"
                id={`col-${l.key}`}
                onDragOver={l.key === 'queued' ? onDragOver : undefined}
                onDrop={l.key === 'queued' ? (e) => e.preventDefault() : undefined}
              >
                {shown.length ? (
                  shown.map((c) => (
                    <JobCardView
                      actions={{
                        act,
                        variants: showVariants,
                        log: showLog,
                        clip: showClip,
                        review,
                        selected: picked.has(c.file),
                        onSelect: (card, on) =>
                          setPicked((p) => {
                            const next = new Map(p);
                            if (on) next.set(card.file, card.lane);
                            else next.delete(card.file);
                            return next;
                          }),
                        onDragStart: (card) => {
                          dragFile.current = card.file;
                        },
                        onDragEnd: () => void onDragEnd(),
                      }}
                      card={c}
                      key={c.file}
                    />
                  ))
                ) : (
                  <ColEmpty text={cards.length ? 'Nothing here matches the filter.' : l.empty} />
                )}
              </div>
            </div>
          );
        })}
      </div>
    </section>
  );
}

/* Escape a filename for use inside a CSS attribute selector. */
const cssEscape = (s: string) => s.replace(/["\\]/g, '\\$&');

/* --- the log panel: through the command surface, so the popover gets it too,
       and a running job's log keeps updating while you watch. --------------- */
function LogBody({ card }: { card: JobCard }) {
  const [log, setLog] = useState<JobLog | null>(null);
  const [error, setError] = useState('');
  const pre = useRef<HTMLPreElement | null>(null);

  useEffect(() => {
    let alive = true;
    const load = async () => {
      try {
        const r = await call<JobLog>('get_job_log', { id: card.id, host: card.host, lines: 300 });
        if (!alive) return;
        const el = pre.current;
        const atBottom = el ? el.scrollTop + el.clientHeight >= el.scrollHeight - 30 : true;
        setLog(r);
        setError('');
        if (el && atBottom) requestAnimationFrame(() => (el.scrollTop = el.scrollHeight));
      } catch (e) {
        if (alive) setError(errText(e));
      }
    };
    void load();
    // A running job's log grows; a finished one doesn't, so don't poll it.
    const t = card.lane === 'running' ? setInterval(load, 4000) : undefined;
    return () => {
      alive = false;
      if (t) clearInterval(t);
    };
  }, [card.id, card.host, card.lane]);

  return (
    <>
      <div className="eta">
        {log?.total
          ? `step ${log.step} of ${log.total} · ${log.percent}%`
          : card.lane === 'running'
            ? 'no step counter in this log yet'
            : log?.path || ''}
      </div>
      <pre ref={pre}>{error || (log ? log.lines.join('\n') : 'loading…')}</pre>
    </>
  );
}

/* --- variants ------------------------------------------------------------
   The recommendations come from the backend (jobs.rs) so both surfaces offer the
   same set, and each one is a complete job ready to queue. */
function VariantsBody({
  card,
  onQueued,
  panelClose,
  show,
}: {
  card: JobCard;
  onQueued: () => Promise<void>;
  panelClose: () => void;
  show: Show;
}) {
  const [variants, setVariants] = useState<Variant[] | null>(null);
  const [error, setError] = useState('');
  const [checked, setChecked] = useState<Set<number>>(new Set());
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        const r = await call<{ variants: Variant[] }>('job_variants', { lane: card.lane, file: card.file });
        if (!alive) return;
        setVariants(r.variants || []);
        // the other delivery sizes are the request that actually arrives
        setChecked(new Set((r.variants || []).map((v, i) => (v.group === 'size' ? i : -1)).filter((i) => i >= 0)));
      } catch (e) {
        if (alive) setError(errText(e));
      }
    })();
    return () => {
      alive = false;
    };
  }, [card.file, card.lane]);

  const groups = useMemo(() => {
    const g: Record<string, { v: Variant; i: number }[]> = {};
    (variants ?? []).forEach((v, i) => {
      (g[v.group] ||= []).push({ v, i });
    });
    return g;
  }, [variants]);

  const queue = async () => {
    setBusy(true);
    let queued = 0;
    let failed = 0;
    for (const i of checked) {
      const v = variants?.[i];
      if (!v) continue;
      try {
        await call('enqueue_job', { job: v.job });
        queued++;
      } catch (e) {
        failed++;
        show(errText(e), 'bad');
      }
    }
    setBusy(false);
    setChecked(new Set());
    if (queued) {
      show(
        `Queued ${queued} variant${queued === 1 ? '' : 's'}${failed ? ` · ${failed} failed` : ''}`,
        failed ? 'bad' : 'good'
      );
    }
    await onQueued();
    if (!failed) panelClose();
  };

  return (
    <>
      <p className="sub">
        Tick what you want and the farm renders them next. Nothing here changes the original.
      </p>
      {!variants && !error && <Looking text="Working out what to offer…" />}
      {error && <div className="step">{error}</div>}
      {Object.keys(GROUP_TITLE)
        .filter((g) => groups[g])
        .map((g) => (
          <div className="vgrp" key={g}>
            <h2>{GROUP_TITLE[g]}</h2>
            {groups[g]!.map(({ v, i }) => (
              <label className="vrow" key={i}>
                <input
                  checked={checked.has(i)}
                  onChange={(e) => {
                    // Read the event NOW: React nulls currentTarget once this
                    // handler returns, and a state updater runs after that.
                    const on = e.currentTarget.checked;
                    setChecked((s) => {
                      const next = new Set(s);
                      if (on) next.add(i);
                      else next.delete(i);
                      return next;
                    });
                  }}
                  type="checkbox"
                />
                <span>
                  <div className="l">{v.label}</div>
                  <div className="w">{v.why}</div>
                  <div className="s">
                    {v.job.width}×{v.job.height}
                    {v.job.sweep && v.job.sweep > 1 ? ` · ${v.job.sweep} seeds` : ` · seed ${v.job.seed}`}
                    {v.job.mode === 'test' ? ' · proof still' : ''}
                  </div>
                </span>
              </label>
            ))}
          </div>
        ))}
      {/* the footer lives in the panel chrome; this keeps it in sync */}
      <VariantsFoot busy={busy} n={checked.size} onQueue={queue} />
    </>
  );
}

/* The panel's foot is part of the panel chrome in the vanilla UI (#p-foot with
   #p-queue and #p-count). Rendered here, inside the body, so the count and the
   button stay owned by the same state — the ids are preserved. */
function VariantsFoot({ n, onQueue, busy }: { n: number; onQueue: () => void; busy: boolean }) {
  return (
    <div className="panel-foot" style={{ borderTop: 0, paddingLeft: 0, paddingRight: 0 }}>
      <button className="btn pri" disabled={n === 0 || busy} id="p-queue" onClick={onQueue} type="button">
        {busy ? (
          <>
            <span className="spin" />
            Queueing…
          </>
        ) : (
          'Queue selected'
        )}
      </button>
      <span className="grow" />
      <span className="hint" id="p-count" style={{ margin: 0 }}>
        {n} selected
      </span>
    </div>
  );
}

/* --- the composer ------------------------------------------------------- */
function Composer({
  assets,
  onAssets,
  onQueued,
  presets: presetsFromConfig,
  reachable,
  reloadConfig,
  show,
}: {
  assets: Assets;
  onAssets: () => Promise<void>;
  onQueued: () => Promise<void>;
  presets: Preset[];
  reachable: boolean;
  reloadConfig: () => void;
  show: Show;
}) {
  const [open, setOpen] = useState(false);
  // Saving a preset returns the new list; use it straight away rather than
  // waiting for the next config poll, so the picker appears when you save.
  const [presets, setPresets] = useState<Preset[]>(presetsFromConfig);
  // Config is authoritative when it has presets; an empty list is treated as
  // "nothing to say" so a save isn't undone by the next config poll.
  useEffect(() => {
    if (presetsFromConfig.length) setPresets(presetsFromConfig);
  }, [presetsFromConfig]);
  const [kind, setKind] = useState('t2v');
  const [prompt, setPrompt] = useState('');
  const [id, setId] = useState('');
  const [size, setSize] = useState('1080x1920');
  const [mode, setMode] = useState('hero');
  const [sweep, setSweep] = useState('0');
  const [seed, setSeed] = useState('42');
  const [frames, setFrames] = useState('97');
  const [hi, setHi] = useState(false);
  const [image, setImage] = useState('');
  const [lora, setLora] = useState('');
  const [loraScale, setLoraScale] = useState('1.0');
  const [still, setStill] = useState('');
  const [preset, setPreset] = useState('');
  const [dropOver, setDropOver] = useState(false);
  const fileRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    if (assets.images.length && !image) setImage(assets.images[0]!);
    if (assets.loras.length && !lora) setLora(assets.loras[0]!);
  }, [assets, image, lora]);

  const read = (): NewJob => {
    const [w, h] = size.split('x').map(Number);
    return {
      id: id.trim(),
      prompt: prompt.trim(),
      kind,
      mode,
      image: kind === 't2v' ? '' : image,
      lora: kind === 'lora_i2v' ? lora : '',
      lora_scale: loraScale || '1.0',
      still_prompt: kind === 'lora_i2v' ? still.trim() : '',
      width: w ?? 1080,
      height: h ?? 1920,
      frames: parseInt(frames, 10) || 97,
      seed: parseInt(seed, 10) || 42,
      fps: 24,
      perf: '',
      extra: '',
      priority: hi ? 'high' : 'normal',
      min_ram_gb: 0,
      sweep: parseInt(sweep, 10) || 0,
      member: '',
      run: '',
    };
  };

  const write = (j: Partial<NewJob>) => {
    setKind(j.kind || 't2v');
    setPrompt(j.prompt || '');
    setSize(`${j.width || 1080}x${j.height || 1920}`);
    setMode(j.mode || 'hero');
    setSweep(String(j.sweep || 0));
    setSeed(String(j.seed ?? 42));
    setFrames(String(j.frames || 97));
    setHi(j.priority === 'high');
    setLoraScale(j.lora_scale || '1.0');
    setStill(j.still_prompt || '');
    if (j.image) setImage(j.image);
    if (j.lora) setLora(j.lora);
  };

  const upload = async (file: File) => {
    const url = uploadUrl(file.name);
    if (!url) {
      show('Turn the web gateway on to upload from here (or drop it in the assets folder).', 'bad');
      return;
    }
    if (!/^image\//.test(file.type || '')) {
      show('That’s not an image.', 'bad');
      return;
    }
    show(`Uploading ${file.name}…`);
    try {
      const res = await fetch(url, { method: 'POST', body: file });
      const out = (await res.json()) as { ok: boolean; data?: { name: string; message: string }; error?: string };
      if (!out.ok) throw new Error(out.error || 'upload failed');
      await onAssets();
      setKind('i2v');
      setImage(out.data!.name);
      show(out.data!.message, 'good');
    } catch (e) {
      show(errText(e), 'bad');
    }
  };

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    const job = read();
    if (!job.prompt) {
      show('A prompt is the one thing the render can’t guess.', 'bad');
      return;
    }
    if (job.kind !== 't2v' && !job.image) {
      show('Pick a starting image (or upload one) for an image-to-video job.', 'bad');
      return;
    }
    if (job.kind === 'lora_i2v' && !job.lora) {
      show('Pick a LoRA, or switch the job type back.', 'bad');
      return;
    }
    try {
      const r = await call<{ message: string }>('enqueue_job', { job });
      show(r?.message || 'Queued.', 'good');
      setPrompt('');
      setId('');
      setOpen(false);
      await onQueued();
    } catch (err) {
      show(errText(err), 'bad');
    }
  };

  return (
    <details className="composer card" id="composer" onToggle={(e) => setOpen(e.currentTarget.open)} open={open}>
      <summary>
        <span className="plus">＋</span> Queue a clip
        <span className="sum-hint" id="comp-hint">
          {reachable ? 'prompt → the next free Mac' : 'share not mounted'}
        </span>
      </summary>
      <form autoComplete="off" className="cfg" id="new-job" onSubmit={submit}>
        <div className="f">
          <label htmlFor="n-prompt">Prompt</label>
          <textarea
            id="n-prompt"
            onChange={(e) => setPrompt(e.currentTarget.value)}
            placeholder="storm clouds rolling over a QLD tile roof, cinematic"
            rows={2}
            value={prompt}
          />
        </div>
        <div className="two">
          <div className="f">
            <label htmlFor="n-kind">Job type</label>
            <select id="n-kind" onChange={(e) => setKind(e.currentTarget.value)} value={kind}>
              <option value="t2v">text → video</option>
              <option value="i2v">image → video</option>
              <option value="lora_i2v">LoRA → still → video</option>
            </select>
          </div>
          <div className="f">
            <label htmlFor="n-id">Name</label>
            <input id="n-id" onChange={(e) => setId(e.currentTarget.value)} placeholder="hail_hero" spellCheck={false} value={id} />
          </div>
        </div>

        <div className="f" hidden={kind === 't2v'} id="wrap-image">
          <label htmlFor="n-image">Starting image (in assets/)</label>
          <div className="row-pick">
            <select id="n-image" onChange={(e) => setImage(e.currentTarget.value)} value={image}>
              {assets.images.length === 0 && <option value="">nothing in assets/ yet — upload one</option>}
              {assets.images.map((n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ))}
            </select>
            <button className="btn sm" id="n-upload" onClick={() => fileRef.current?.click()} type="button">
              Upload…
            </button>
          </div>
          <div
            className={`drop${dropOver ? ' over' : ''}`}
            id="n-drop"
            onDragLeave={() => setDropOver(false)}
            onDragOver={(e) => {
              e.preventDefault();
              setDropOver(true);
            }}
            onDrop={(e) => {
              e.preventDefault();
              setDropOver(false);
              const f = e.dataTransfer?.files?.[0];
              if (f) void upload(f);
            }}
          >
            Drop an image here to put it on the share
          </div>
          <input
            accept="image/*"
            hidden
            id="n-file"
            onChange={(e) => {
              const f = e.currentTarget.files?.[0];
              if (f) void upload(f);
              e.currentTarget.value = '';
            }}
            ref={fileRef}
            type="file"
          />
        </div>

        <div className="two" hidden={kind !== 'lora_i2v'} id="wrap-lora">
          <div className="f">
            <label htmlFor="n-lora">LoRA</label>
            <select id="n-lora" onChange={(e) => setLora(e.currentTarget.value)} value={lora}>
              {assets.loras.length === 0 && <option value="">no LoRAs on the share</option>}
              {assets.loras.map((n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ))}
            </select>
          </div>
          <div className="f">
            <label htmlFor="n-lora-scale">LoRA strength</label>
            <input
              id="n-lora-scale"
              max="2"
              min="0"
              onChange={(e) => setLoraScale(e.currentTarget.value)}
              step="0.05"
              type="number"
              value={loraScale}
            />
          </div>
        </div>

        <div className="f" hidden={kind !== 'lora_i2v'} id="wrap-still">
          <label htmlFor="n-still">Still prompt (use the trigger word)</label>
          <input id="n-still" onChange={(e) => setStill(e.currentTarget.value)} placeholder="eljhwd man tasting hail, kitchen" value={still} />
        </div>

        <div className="two">
          <div className="f">
            <label htmlFor="n-size">Delivery size</label>
            <select id="n-size" onChange={(e) => setSize(e.currentTarget.value)} value={size}>
              {SIZES.map((s) => (
                <option key={s.v} value={s.v}>
                  {s.l}
                </option>
              ))}
            </select>
          </div>
          <div className="f">
            <label htmlFor="n-mode">First pass</label>
            <select id="n-mode" onChange={(e) => setMode(e.currentTarget.value)} value={mode}>
              <option value="hero">hero — full video render</option>
              <option value="test">test — cheap proof still first</option>
            </select>
          </div>
        </div>
        <div className="two">
          <div className="f">
            <label htmlFor="n-sweep">Takes</label>
            <select id="n-sweep" onChange={(e) => setSweep(e.currentTarget.value)} value={sweep}>
              <option value="0">1 — single render</option>
              <option value="2">2 seeds</option>
              <option value="4">4 seeds — split across the farm</option>
              <option value="8">8 seeds</option>
            </select>
          </div>
          <div className="f">
            <label htmlFor="n-seed">Seed</label>
            <input id="n-seed" onChange={(e) => setSeed(e.currentTarget.value)} type="number" value={seed} />
          </div>
        </div>
        <div className="two">
          <div className="f">
            <label htmlFor="n-frames">Frames (8k+1)</label>
            <input
              id="n-frames"
              min="9"
              onChange={(e) => setFrames(e.currentTarget.value)}
              step="8"
              type="number"
              value={frames}
            />
          </div>
          <div className="f" />
        </div>

        <label className="ck">
          <input checked={hi} id="n-hi" onChange={(e) => setHi(e.currentTarget.checked)} type="checkbox" />{' '}
          <span>Jump the queue (priority lane)</span>
        </label>

        <div className="f" hidden={presets.length === 0} id="wrap-presets">
          <label htmlFor="n-preset">Saved setups</label>
          <div className="row-pick">
            <select
              id="n-preset"
              onChange={(e) => {
                setPreset(e.currentTarget.value);
                const p = presets.find((x) => x.name === e.currentTarget.value);
                if (p) write(p.job);
              }}
              value={preset}
            >
              <option value="">—</option>
              {presets.map((p) => (
                <option key={p.name} value={p.name}>
                  {p.name}
                </option>
              ))}
            </select>
            <button
              className="btn sm"
              id="n-preset-del"
              onClick={async () => {
                if (!preset) return;
                try {
                  const r = await call<{ message: string; presets?: Preset[] }>('delete_preset', { name: preset });
                  show(r.message, 'good');
                  if (r.presets) setPresets(r.presets);
                  setPreset('');
                  reloadConfig();
                } catch (e) {
                  show(errText(e), 'bad');
                }
              }}
              type="button"
            >
              Delete
            </button>
          </div>
        </div>

        <div className="bar-actions" style={{ marginTop: 4 }}>
          <button className="btn pri" id="n-go" type="submit">
            Queue it
          </button>
          <button
            className="btn sm"
            id="n-save-preset"
            onClick={async () => {
              const name = window.prompt('Name this setup (prompt is not saved):', id || '');
              if (!name) return;
              const job = { ...read(), prompt: '' }; // a preset is a shape, not a shot
              try {
                const r = await call<{ message: string; presets?: Preset[] }>('save_preset', { name, job });
                show(r.message, 'good');
                if (r.presets) setPresets(r.presets);
                setPreset(name);
                reloadConfig();
              } catch (e) {
                show(errText(e), 'bad');
              }
            }}
            type="button"
          >
            Save as preset
          </button>
          <span className="grow" />
          <span className="hint" id="n-where" style={{ margin: 0 }} />
        </div>
      </form>
    </details>
  );
}

/* --- the overnight planner ----------------------------------------------
   The "paste the shot list and go home" path. It tells you the size of the night
   BEFORE you commit it, because 200 jobs on 4 Macs is a decision. */
function Planner({ onQueued, show }: { onQueued: () => Promise<void>; show: Show }) {
  const [open, setOpen] = useState(false);
  const [prompts, setPrompts] = useState('');
  const [name, setName] = useState('');
  const [mode, setMode] = useState('hero');
  const [sizes, setSizes] = useState<Set<string>>(new Set(['1080x1920']));
  const [seeds, setSeeds] = useState('1');
  const [frames, setFrames] = useState('97');
  const [macs, setMacs] = useState(1);
  const [busy, setBusy] = useState(false);

  // The planner's divisor: Macs that could actually take a job.
  useEffect(() => {
    void (async () => {
      try {
        const r = await call<{ members: { worker: boolean; state: string }[] }>('get_members');
        setMacs(Math.max(1, (r.members || []).filter((m) => m.worker && m.state !== 'offline').length));
      } catch {
        setMacs(1);
      }
    })();
  }, [open]);

  const list = prompts.split('\n').map((x) => x.trim()).filter(Boolean);
  const jobs = list.length * Math.max(1, sizes.size) * (parseInt(seeds, 10) || 1);
  const per = mode === 'test' ? 90 : 1800;
  const wall = Math.round((jobs * per) / macs);

  const summary = !list.length
    ? 'Paste one prompt per line to see the size of the night.'
    : `${list.length} prompt(s) × ${Math.max(1, sizes.size)} size(s) × ${parseInt(seeds, 10) || 1} take(s) = ${jobs} job(s) · roughly ${secs(wall)} across ${macs} Mac(s)${mode === 'test' ? ' (proof stills — cheap)' : ''}`;

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!list.length) {
      show('Paste at least one prompt — one per line.', 'bad');
      return;
    }
    setBusy(true);
    try {
      const r = await call<{ message: string }>('plan_run', {
        plan: {
          run: name.trim(),
          prompts: prompts.split('\n'),
          sizes: [...sizes],
          seeds: parseInt(seeds, 10) || 1,
          frames: parseInt(frames, 10) || 97,
          mode,
          note: '',
        },
      });
      show(r.message, 'good');
      setPrompts('');
      setOpen(false);
      await onQueued();
    } catch (err) {
      show(errText(err), 'bad');
    } finally {
      setBusy(false);
    }
  };

  return (
    <details className="composer card" id="planner" onToggle={(e) => setOpen(e.currentTarget.open)} open={open}>
      <summary>
        <span className="plus">🌙</span> Plan an overnight run
        <span className="sum-hint" id="plan-hint">
          paste a shot list, go home
        </span>
      </summary>
      <form autoComplete="off" className="cfg" id="new-run" onSubmit={submit}>
        <div className="f">
          <label htmlFor="r-prompts">Shot list — one prompt per line</label>
          <textarea
            id="r-prompts"
            onChange={(e) => setPrompts(e.currentTarget.value)}
            placeholder={'storm clouds over a QLD tile roof, cinematic\nhail bouncing off a driveway, slow motion\nassessor on a roof with a tablet'}
            rows={5}
            value={prompts}
          />
        </div>
        <div className="two">
          <div className="f">
            <label htmlFor="r-name">Run name</label>
            <input id="r-name" onChange={(e) => setName(e.currentTarget.value)} placeholder="overnight" spellCheck={false} value={name} />
          </div>
          <div className="f">
            <label htmlFor="r-mode">How to spend the night</label>
            <select id="r-mode" onChange={(e) => setMode(e.currentTarget.value)} value={mode}>
              <option value="hero">hero — full renders of everything</option>
              <option value="test">proofs — cheap stills to cherry-pick</option>
            </select>
          </div>
        </div>
        <div className="f">
          <label>Delivery sizes</label>
          <div className="chips" id="r-sizes">
            {[
              ['1080x1920', '9:16'],
              ['1080x1080', '1:1'],
              ['1920x1080', '16:9'],
              ['1080x1350', '4:5'],
            ].map(([v, l]) => (
              <label className="chip" key={v}>
                <input
                  checked={sizes.has(v!)}
                  onChange={(e) => {
                    const on = e.currentTarget.checked;   // see the note above
                    setSizes((s) => {
                      const next = new Set(s);
                      if (on) next.add(v!);
                      else next.delete(v!);
                      return next;
                    });
                  }}
                  type="checkbox"
                  value={v}
                />{' '}
                {l}
              </label>
            ))}
          </div>
        </div>
        <div className="two">
          <div className="f">
            <label htmlFor="r-seeds">Takes per prompt</label>
            <select id="r-seeds" onChange={(e) => setSeeds(e.currentTarget.value)} value={seeds}>
              {['1', '2', '3', '4', '6'].map((n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ))}
            </select>
          </div>
          <div className="f">
            <label htmlFor="r-frames">Frames (8k+1)</label>
            <input id="r-frames" min="9" onChange={(e) => setFrames(e.currentTarget.value)} step="8" type="number" value={frames} />
          </div>
        </div>
        <div className="plan-sum" id="r-sum">
          {summary}
        </div>
        <div className="bar-actions" style={{ marginTop: 4 }}>
          <button className="btn pri" disabled={busy} id="r-go" type="submit">
            {busy ? (
              <>
                <span className="spin" />
                Queueing…
              </>
            ) : (
              'Queue the night'
            )}
          </button>
          <span className="grow" />
          <span className="hint" id="r-auto-note" style={{ margin: 0 }} />
        </div>
      </form>
    </details>
  );
}
