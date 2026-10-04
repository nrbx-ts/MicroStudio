// mirrors crates/microstudio-runtime/src/protocol.rs; keep both in step

export const PROTOCOL_VERSION = 1;

export interface RuntimeRequest {
  id: number;
  method: string;
  params?: unknown;
}

export interface RuntimeErrorInfo {
  message: string;
  location?: string;
  traceback?: string;
}

export interface RuntimeResponse {
  id: number;
  ok: boolean;
  result?: unknown;
  error?: RuntimeErrorInfo;
}

export interface RuntimeNotification {
  method: string;
  params?: Record<string, unknown>;
}

export interface InstanceJson {
  id: number;
  name: string;
  className: string;
  path: string;
  childCount?: number;
  children?: InstanceJson[];
  attributes?: Record<string, { type: string; value: string }>;
}

export interface TreeResult {
  root: InstanceJson;
  time: number;
}

export interface StatsResult {
  time: number;
  clock: "virtual" | "real";
  pendingWork: number;
  instances: number;
  logs: number;
  errors: number;
}

export interface HelloResult {
  protocol: number;
  clock: "virtual" | "real";
  // e.g. "Luau 0.740"
  luau: string;
  // every service the runtime can hand back, in the API dump's set
  services: string[];
}

export type LogLevel = "print" | "warn" | "error" | "info";

// auto: repl mode, echoes bare expressions instead of failing
export type EvalMode = "statement" | "expression" | "auto";

export interface LogNotification {
  level: LogLevel;
  text: string;
  source?: string | null;
}

export interface ScriptErrorNotification {
  location: string;
  message: string;
  traceback: string;
  rendered: string;
}

// parents before children
export interface PlaceEntry {
  path: string;
  className: string;
  source?: string;
  // source file a script or module came from, when one exists
  file?: string;
  properties?: Record<string, unknown>;
}

export interface LoadTreeResult {
  created: string[];
}

export interface RunScriptsResult {
  scripts: string[];
}
