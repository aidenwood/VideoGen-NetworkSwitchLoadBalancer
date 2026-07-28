/* The shapes Core::dispatch returns. Mirrors the serde structs in
   src-tauri/src/{lib,jobs}.rs — when one changes, both change. Kept as plain
   types (not zod) because the backend is in-process: there's no untrusted
   boundary here, only a contract to keep honest. */

export type Counts = { queued: number; running: number; done: number; failed: number };

export type FarmEvent = { kind: string; id: string; host: string; ts: string };

export type WorkerRow = { host: string; state: string; job: string; age_secs: number };

export type FarmState = {
  root: string;
  counts: Counts;
  workers: WorkerRow[];
  events: FarmEvent[];
  rev: number;
  surface_host: string;
};

export type Lane = 'queued' | 'running' | 'done' | 'failed';

export type JobCard = {
  file: string;
  path: string;
  lane: Lane;
  id: string;
  stamp: string;
  priority: 'high' | 'normal';
  position: number;
  prompt: string;
  kind: string;
  mode: string;
  width: number;
  height: number;
  frames: number;
  seed: number;
  fps: number;
  perf: string;
  lora: string;
  image: string;
  min_ram_gb: number;
  oom_retry: number;
  host: string;
  age_secs: number;
  mp4: string;
  mp4_mb: number;
  proof: string;
  log: string;
  rc: string;
  duration_secs: number;
  peak_mem_gb: number;
  aspect: string;
  member: string;
  run: string;
  retry: number;
  review: '' | 'approved' | 'retake';
  review_by: string;
  review_note: string;
  est_secs: number;
  eta_secs: number;
};

export type Board = {
  root: string;
  reachable: boolean;
  queued: JobCard[];
  running: JobCard[];
  done: JobCard[];
  failed: JobCard[];
  totals: Record<string, number>;
};

export type RunRow = {
  run: string;
  note: string;
  by: string;
  created_ts: number;
  planned: number;
  proof_first: boolean;
  queued: number;
  running: number;
  done: number;
  failed: number;
  render_secs: number;
  finished: boolean;
};

export type BoardResponse = {
  board: Board;
  share_url: string;
  held: number;
  member: string;
  runs: RunRow[];
  is_coordinator: boolean;
};

export type Member = {
  host: string;
  member: string;
  model: string;
  ram_gb: number;
  role: string;
  perf: string;
  state: string;
  detail: string;
  job: string;
  job_prompt: string;
  elapsed_secs: number;
  last_seen_secs: number;
  gateway: string;
  free_pct: number;
  pressure: number;
  swap_mb: number;
  budget_gb: number;
  done_count: number;
  is_you: boolean;
  app: boolean;
  worker: boolean;
};

export type MembersResponse = {
  you: string;
  member: string;
  reachable: boolean;
  members: Member[];
};

export type Check = {
  id: string;
  stage: number;
  stage_label: string;
  label: string;
  status: 'ok' | 'warn' | 'fail';
  detail: string;
  fix: string;
  action: string;
  action_label: string;
};

export type VerifyReport = {
  host: string;
  root: string;
  is_coordinator: boolean;
  checks: Check[];
  workers: WorkerRow[];
  ok: number;
  warn: number;
  fail: number;
  ready: boolean;
};

export type SetupStep = {
  id: string;
  title: string;
  body: string;
  done: boolean;
  detail: string;
  action: string;
  action_label: string;
  manual: boolean;
};

export type SetupResponse = {
  host: string;
  role: string;
  root: string;
  share_url: string;
  steps: SetupStep[];
  all_done: boolean;
  wizard_done: boolean;
};

export type Config = {
  coordinator: string;
  share_path: string;
  share_name: string;
  perf: string;
  min_free_gb: number;
  ltx_dir: string;
  lora_dir: string;
  repo_dir: string;
  role: string;
  wizard_done: boolean;
  member: string;
  web_enabled: boolean;
  web_port: number;
  web_lan: boolean;
  web_open_on_launch: boolean;
  autopilot: boolean;
  autopilot_retry: number;
  stale_min: number;
  fail_streak: number;
  presets: Preset[];
};

export type Preset = { name: string; job: Partial<NewJob> };

export type NewJob = {
  id: string;
  prompt: string;
  kind: string;
  mode: string;
  image: string;
  lora: string;
  lora_scale: string;
  still_prompt: string;
  width: number;
  height: number;
  frames: number;
  seed: number;
  fps: number;
  perf: string;
  extra: string;
  priority: string;
  min_ram_gb: number;
  sweep: number;
  member: string;
  run: string;
};

export type ConfigResponse = {
  config: Config;
  resolved: { root: string; ltx_dir: string; share_url: string };
  host: string;
  config_file: string;
  gateway: import('./api').GatewayInfo | null;
  presets: Preset[];
};

export type Variant = {
  group: 'size' | 'prompt' | 'seed' | 'quality';
  label: string;
  why: string;
  job: Partial<NewJob> & { width: number; height: number; seed: number; sweep: number; mode: string };
};

export type Proof = {
  id: string;
  path: string;
  seed: number;
  age_secs: number;
  review: string;
  prompt: string;
  width: number;
  height: number;
  done_file: string;
  rendered: boolean;
};

export type ProofsResponse = { proofs: Proof[]; clips: JobCard[]; reachable: boolean };

export type HostStat = {
  host: string;
  clips: number;
  secs: number;
  avg_secs: number;
  clips_24h: number;
  peak_mem_gb: number;
  budget_gb: number;
};

export type SizeStat = {
  label: string;
  width: number;
  height: number;
  frames: number;
  mode: string;
  clips: number;
  avg_secs: number;
};

export type Stats = {
  clips: number;
  clips_24h: number;
  secs_24h: number;
  avg_secs: number;
  per_host: HostStat[];
  by_size: SizeStat[];
  over_budget: number;
  sample: number;
};

export type StatsResponse = { stats: Stats; members: Member[]; reachable: boolean };

export type ConfKey = {
  key: string;
  label: string;
  help: string;
  kind: 'int' | 'choice' | 'text';
  choices: string[];
  min: number;
  max: number;
  value: string;
};

export type FarmConfResponse = { path: string; exists: boolean; keys: ConfKey[] };

export type AutopilotResponse = {
  on: boolean;
  you: string;
  supervisor: string;
  policy: { retry: number; stale_min: number; fail_streak: number };
  held: number;
  log: string[];
};

export type RunReport = {
  run: string;
  done: JobCard[];
  failed: JobCard[];
  queued: JobCard[];
  running: JobCard[];
  counts: {
    done: number;
    failed: number;
    queued: number;
    running: number;
    approved: number;
    retake: number;
  };
  render_secs: number;
  per_host: { host: string; clips: number }[];
  finished: boolean;
};

export type JobLog = {
  path: string;
  lines: string[];
  step: number;
  total: number;
  percent: number;
};

export type Assets = { images: string[]; loras: string[] };

export type Message = { message: string };
