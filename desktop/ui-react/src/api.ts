/* ===========================================================================
   The one command surface, from the client side.
   ---------------------------------------------------------------------------
   TWO SURFACES, ONE PAGE. Inside the menubar app this talks to Tauri; served
   over HTTP by the gateway (src-tauri/src/web.rs) the same code POSTs to
   /api/invoke. Both land in Core::dispatch, so a feature cannot exist in one
   place and be missing in the other.

   Everything funnels through call(). Anything that throws gets surfaced — a
   silently swallowed error is how a dead button passes for a working one.
   =========================================================================== */

import type { Command } from './commands';

type TauriInvoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

declare global {
  interface Window {
    __TAURI__?: { core?: { invoke?: TauriInvoke } };
    __openTab?: (tab: string) => void;
  }
}

const rawInvoke: TauriInvoke | undefined = window.__TAURI__?.core?.invoke;

/** True when this page is being served by the gateway rather than by Tauri. */
export const WEB = !rawInvoke;

/** The gateway key, when the page was opened from a team link. */
export const KEY = (() => {
  try {
    return new URLSearchParams(location.search).get('k') || '';
  } catch {
    return '';
  }
})();

/** file:// (opened by hand, or a test harness) has nothing serving /file. */
export const SERVED_OVER_HTTP = /^https?:/.test(location.protocol);

export async function call<T = unknown>(cmd: Command, args?: Record<string, unknown>): Promise<T> {
  const payload = args ?? {};
  if (rawInvoke) return (await rawInvoke('bridge', { cmd, args: payload })) as T;

  const head: Record<string, string> = { 'Content-Type': 'application/json' };
  if (KEY) head['X-Farm-Key'] = KEY;
  const res = await fetch('/api/invoke', {
    method: 'POST',
    headers: head,
    body: JSON.stringify({ cmd, args: payload }),
  });
  if (res.status === 403) {
    throw new Error(
      'This link is missing its key — open the farm link your team shared (it ends in ?k=…).'
    );
  }
  if (!res.ok) throw new Error(`The farm gateway answered ${res.status}.`);
  const out = (await res.json()) as { ok: boolean; data?: T; error?: string };
  if (!out.ok) throw new Error(out.error || 'The farm couldn’t do that.');
  return out.data as T;
}

export const errText = (e: unknown): string =>
  e instanceof Error ? e.message : String(e);

/* --- media ---------------------------------------------------------------
   Renders, posters and logs come off the share through the gateway, so a
   browser on somebody else's desk can watch a clip without mounting SMB — and
   the popover can show thumbnails too, by talking to its own local gateway. */

export type GatewayInfo = {
  running: boolean;
  enabled: boolean;
  port: number;
  lan: boolean;
  local_url: string;
  lan_url: string;
  token: string;
};

let gateway: GatewayInfo | null = null;
export const setGateway = (g: GatewayInfo | null) => {
  gateway = g;
};
export const getGateway = () => gateway;

function mediaBase(): { origin: string; key: string } | null {
  if (WEB) return SERVED_OVER_HTTP ? { origin: '', key: KEY } : null;
  if (gateway?.running && gateway.port) {
    return { origin: `http://127.0.0.1:${gateway.port}`, key: gateway.token || '' };
  }
  return null; // no gateway: no media, and the UI hides those buttons
}

function mediaUrl(route: string, path: string, extra = ''): string | null {
  const b = mediaBase();
  if (!b) return null;
  return (
    b.origin +
    route +
    '?path=' +
    encodeURIComponent(path) +
    extra +
    (b.key ? '&k=' + encodeURIComponent(b.key) : '')
  );
}

export const fileUrl = (path: string, dl = false) => mediaUrl('/file', path, dl ? '&dl=1' : '');
export const posterUrl = (path: string) => mediaUrl('/poster', path);
export const uploadUrl = (name: string) => {
  const b = mediaBase();
  if (!b) return null;
  return (
    b.origin + '/upload?name=' + encodeURIComponent(name) + (b.key ? '&k=' + encodeURIComponent(b.key) : '')
  );
};

/* --- formatting ---------------------------------------------------------- */

export const secs = (n: number | undefined): string => {
  const v = Math.max(0, Math.round(n || 0));
  if (v < 60) return `${v}s`;
  if (v < 3600) return `${Math.floor(v / 60)}m ${v % 60}s`;
  return `${Math.floor(v / 3600)}h ${Math.floor((v % 3600) / 60)}m`;
};
