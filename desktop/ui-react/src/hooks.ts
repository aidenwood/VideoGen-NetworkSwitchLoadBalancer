/* Shared state for both surfaces.
   The vanilla UI kept module-level variables and setInterval; React keeps the
   same polling cadence but hangs it off hooks so a view re-renders when its own
   data lands instead of every view reaching into the DOM. */
import { useCallback, useEffect, useRef, useState } from 'react';
import { call, errText } from './api';
import type { Command } from './commands';

/** Poll a command while `active`, and expose the last good value + last error. */
export function usePoll<T>(
  cmd: Command,
  ms: number,
  active: boolean,
  args?: Record<string, unknown>
) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState('');
  const busy = useRef(false);
  const argsRef = useRef(args);
  argsRef.current = args;

  const refresh = useCallback(async () => {
    if (busy.current) return;
    busy.current = true;
    try {
      const out = await call<T>(cmd, argsRef.current);
      setData(out);
      setError('');
    } catch (e) {
      setError(errText(e));
    } finally {
      busy.current = false;
    }
  }, [cmd]);

  useEffect(() => {
    if (!active) return;
    void refresh();
    const t = setInterval(() => {
      // Don't poll a view nobody is looking at, and don't poll a backgrounded
      // window — a phone left open on the board shouldn't hammer the share.
      if (document.hasFocus()) void refresh();
    }, ms);
    return () => clearInterval(t);
  }, [active, ms, refresh]);

  return { data, error, refresh, setData };
}

/**
 * The 2s heartbeat every surface shares. `rev` moves whenever a setting, role or
 * name changes anywhere — in the popover, in another browser tab, on a phone —
 * so each open view can reload itself instead of showing a stale config.
 */
export function useRev(onChange: () => void) {
  const last = useRef<number | null>(null);
  const cb = useRef(onChange);
  cb.current = onChange;
  return useCallback((rev: number | undefined) => {
    if (rev === undefined) return;
    if (last.current !== null && rev !== last.current) cb.current();
    last.current = rev;
  }, []);
}

export type ToastKind = 'good' | 'bad' | '';
export type Toast = { msg: string; kind: ToastKind; visible: boolean };

export function useToast() {
  const [toast, setToast] = useState<Toast>({ msg: '', kind: '', visible: false });
  const timer = useRef<number | undefined>(undefined);

  const show = useCallback((msg: string, kind: ToastKind = '') => {
    setToast({ msg, kind, visible: true });
    window.clearTimeout(timer.current);
    // Fades out but keeps the text, exactly as the vanilla toast did.
    timer.current = window.setTimeout(
      () => setToast((t) => ({ ...t, visible: false })),
      4600
    );
  }, []);

  useEffect(() => () => window.clearTimeout(timer.current), []);
  return { toast, show };
}

/** Run an async action, reporting failure through the toast. */
export function useAction(show: (m: string, k?: ToastKind) => void) {
  return useCallback(
    async (fn: () => Promise<unknown>, ok?: string) => {
      try {
        const out = (await fn()) as { message?: string } | undefined;
        const msg = ok ?? out?.message;
        if (msg) show(msg, 'good');
        return true;
      } catch (e) {
        show(errText(e), 'bad');
        return false;
      }
    },
    [show]
  );
}
