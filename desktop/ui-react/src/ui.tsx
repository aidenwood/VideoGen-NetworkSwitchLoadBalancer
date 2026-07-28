/* The small shared pieces: buttons that report their own failure, the side
   panel, tags, empty states. Class names and element ids match the vanilla UI so
   the same stylesheet and the same behaviour suite still apply. */
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { errText } from './api';

/* --- spinner + buttons --------------------------------------------------- */

export const Spinner = () => <span className="spin" />;

type BtnProps = {
  label: string;
  onClick: () => void | Promise<unknown>;
  pri?: boolean;
  sm?: boolean;
  title?: string;
  disabled?: boolean;
  /** Text shown while the action is in flight. */
  working?: string;
  /** Kept so the behaviour suite (and the tray) can address a button. */
  id?: string;
  /**
   * Stay disabled after a SUCCESSFUL run, until the parent re-renders this away.
   * Load-bearing for the long installers: `setup.command` takes 20 minutes and a
   * re-enabled button invites a second run of it.
   */
  holdOnSuccess?: boolean;
};

/**
 * A button that disables itself while working and re-enables on failure. Both of
 * those are load-bearing: an enabled button invites a second click and a
 * duplicate render, and a permanently disabled one is a dead end.
 */
export function Btn({
  label,
  onClick,
  pri,
  sm = true,
  title,
  disabled,
  working = 'Working…',
  id,
  holdOnSuccess,
}: BtnProps) {
  const [busy, setBusy] = useState(false);
  const alive = useRef(true);
  useEffect(() => () => { alive.current = false; }, []);
  return (
    <button
      className={`btn${pri ? ' pri' : ''}${sm ? ' sm' : ''}`}
      disabled={busy || disabled}
      id={id}
      onClick={async () => {
        setBusy(true);
        try {
          await onClick();
          // On success, optionally stay busy: the parent will re-render and this
          // button will be gone (step done) or replaced.
          if (alive.current && !holdOnSuccess) setBusy(false);
        } catch {
          // A failure must re-enable — a permanently disabled button is a dead end.
          if (alive.current) setBusy(false);
        }
      }}
      title={title}
      type="button"
    >
      {busy ? (
        <>
          <Spinner />
          {working}
        </>
      ) : (
        label
      )}
    </button>
  );
}

/* --- tags + empty states ------------------------------------------------- */

export const Tag = ({ children, cls }: { children: ReactNode; cls?: string }) => (
  <span className={`tg${cls ? ` ${cls}` : ''}`}>{children}</span>
);

export const Empty = ({ glyph, line, small }: { glyph: string; line: string; small: string }) => (
  <div className="empty">
    <div className="g">{glyph}</div>
    <p>{line}</p>
    <small>{small}</small>
  </div>
);

export const ColEmpty = ({ text }: { text: string }) => <div className="col-empty">{text}</div>;

export const Looking = ({ text }: { text: string }) => (
  <div className="looking">
    <Spinner />
    {text}
  </div>
);

/* --- the side panel -----------------------------------------------------
   A right-docked drawer (bottom sheet on a phone), never a full-screen modal:
   picking variants or watching a clip back is something you do WHILE looking at
   the board, not instead of it. */

type PanelState = {
  title: string;
  body: ReactNode;
  foot?: ReactNode;
} | null;

type PanelApi = {
  open: (p: NonNullable<PanelState>) => void;
  close: () => void;
  /** Replace only the body of an already-open panel (used while a log streams). */
  setBody: (body: ReactNode) => void;
  isOpen: boolean;
};

const PanelCtx = createContext<PanelApi | null>(null);
export const usePanel = () => {
  const ctx = useContext(PanelCtx);
  if (!ctx) throw new Error('usePanel outside PanelHost');
  return ctx;
};

export function PanelHost({ children }: { children: ReactNode }) {
  const [state, setState] = useState<PanelState>(null);

  const api = useMemo<PanelApi>(
    () => ({
      open: (p) => setState(p),
      close: () => setState(null),
      setBody: (body) => setState((s) => (s ? { ...s, body } : s)),
      isOpen: state !== null,
    }),
    [state]
  );

  // Esc closes it, from anywhere.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setState(null);
    };
    addEventListener('keydown', onKey);
    return () => removeEventListener('keydown', onKey);
  }, []);

  // On a wide screen the drawer is docked to the viewport edge, so give the page
  // room rather than letting it cover the numbers you opened it to compare.
  useEffect(() => {
    document.body.classList.toggle('panel-open', state !== null);
  }, [state]);

  return (
    <PanelCtx.Provider value={api}>
      {children}
      <aside aria-hidden={state === null} className={`panel${state ? ' open' : ''}`} id="panel">
        <div className="panel-head">
          <b id="p-title">{state?.title ?? ''}</b>
          <span className="grow" />
          <button aria-label="Close panel" className="btn sm" id="p-close" onClick={() => setState(null)} type="button">
            Close
          </button>
        </div>
        <div className="panel-body" id="p-body">
          {state?.body}
        </div>
        <div className="panel-foot" hidden={!state?.foot} id="p-foot">
          {state?.foot}
        </div>
      </aside>
    </PanelCtx.Provider>
  );
}

/* --- inline error, where the user is looking ---------------------------- */

export const InlineError = ({ e }: { e: unknown }) => (
  <div className="err">
    <b>That didn’t work</b>
    {errText(e)}
  </div>
);

/* --- a segmented control, thumb included -------------------------------- */

export function Seg({
  tabs,
  active,
  onPick,
  label,
  thumbId,
  idPrefix,
  extra,
}: {
  tabs: { key: string; label: string }[];
  active: string;
  onPick: (key: string) => void;
  label: string;
  thumbId: string;
  idPrefix: string;
  extra?: Record<string, ReactNode>;
}) {
  const host = useRef<HTMLDivElement | null>(null);
  const [thumb, setThumb] = useState({ w: 0, x: 0 });

  const move = useCallback(() => {
    const el = host.current?.querySelector<HTMLElement>(`#${idPrefix}${active}`);
    if (!el || !el.offsetWidth) return;
    setThumb({ w: el.offsetWidth, x: el.offsetLeft - 3 });
  }, [active, idPrefix]);

  useEffect(() => {
    move();
    addEventListener('resize', move);
    return () => removeEventListener('resize', move);
  }, [move]);

  return (
    <div aria-label={label} className={`seg${idPrefix === 'rtab-' ? ' sub-seg' : ''}`} ref={host} role="tablist">
      <span className="thumb" id={thumbId} style={{ width: thumb.w, transform: `translateX(${thumb.x}px)` }} />
      {tabs.map((t) => (
        <button
          aria-controls={idPrefix === 'tab-' ? `view-${t.key}` : undefined}
          aria-selected={active === t.key}
          id={`${idPrefix}${t.key}`}
          key={t.key}
          onClick={() => onPick(t.key)}
          role="tab"
        >
          {t.label}
          {extra?.[t.key]}
        </button>
      ))}
    </div>
  );
}
