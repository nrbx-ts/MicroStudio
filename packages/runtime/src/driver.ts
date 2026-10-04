import type { RuntimeNotification } from "./protocol.ts";

// transport seam: stdio sidecar now, napi-rs binding later
export interface RuntimeDriver {
  // virtual in tests (deterministic), real in dev
  readonly clock: "virtual" | "real";

  // e.g. "Luau 0.740"
  readonly luau: string;

  request<T>(method: string, params?: unknown): Promise<T>;

  // unsolicited messages: log, warn, error
  onNotification(handler: (notification: RuntimeNotification) => void): () => void;

  // safe to call more than once
  dispose(): Promise<void>;
}

export class RuntimeCommandError extends Error {
  readonly method: string;
  readonly location: string | undefined;
  readonly traceback: string | undefined;

  constructor(
    method: string,
    message: string,
    location?: string,
    traceback?: string,
  ) {
    super(location ? `${location}: ${message}` : message);
    this.name = "RuntimeCommandError";
    this.method = method;
    this.location = location;
    this.traceback = traceback;
  }
}
