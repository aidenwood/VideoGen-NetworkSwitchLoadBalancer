/* Every command the UI may call.
   ---------------------------------------------------------------------------
   This list is checked against Rust's `COMMANDS` (src-tauri/src/lib.rs) by a
   cargo test AND by `--selftest`, so the two can't drift. Combined with `call()`
   only accepting a `Command`, a typo is a compile error instead of a button that
   spins forever — which is exactly how this app broke four times before. */
export const COMMANDS = [
  'get_state',
  'verify_link',
  'get_config',
  'save_config',
  'run_action',
  'setup_steps',
  'discover_coordinators',
  'set_role',
  'set_coordinator',
  'finish_wizard',
  'pick_repo',
  'mount_share',
  'get_board',
  'job_action',
  'enqueue_job',
  'job_variants',
  'get_members',
  'set_member',
  'get_proofs',
  'set_review',
  'list_assets',
  'get_stats',
  'get_job_log',
  'get_farm_conf',
  'save_farm_conf',
  'farm_action',
  'plan_run',
  'get_runs',
  'get_run_report',
  'get_autopilot',
  'set_autopilot',
  'save_preset',
  'delete_preset',
] as const;

export type Command = (typeof COMMANDS)[number];
