import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { createInterface } from "node:readline";

import { RuntimeCommandError, type RuntimeDriver } from "./driver.ts";
import type {
  HelloResult,
  RuntimeNotification,
  RuntimeResponse,
} from "./protocol.ts";
import { PROTOCOL_VERSION } from "./protocol.ts";
import { resolveRuntimeBinary, type ResolveOptions } from "./resolve-binary.ts";

export interface StdioDriverOptions extends ResolveOptions {
  // virtual: deterministic time; real: task.wait actually waits
  clock?: "virtual" | "real";
  // makes RunService:IsStudio() false, for CI
  headless?: boolean;
  // where simulated services keep their json; without it the sidecar uses ~/.microstudio
  stateDir?: string;
  args?: string[];
  onStderr?: (line: string) => void;
}

interface Pending {
  method: string;
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
}

// newline-delimited JSON-RPC over child stdio; requests matched by id, several in flight
export class StdioRuntimeDriver implements RuntimeDriver {
  readonly clock: "virtual" | "real";
  luau = "Luau (unknown)";

  #child: ChildProcessWithoutNullStreams;
  #pending = new Map<number, Pending>();
  #handlers = new Set<(notification: RuntimeNotification) => void>();
  #nextId = 1;
  #disposed = false;
  #exitError: Error | undefined;

  private constructor(
    child: ChildProcessWithoutNullStreams,
    clock: "virtual" | "real",
  ) {
    this.#child = child;
    this.clock = clock;
    this.#attach();
  }

  static async start(options: StdioDriverOptions = {}): Promise<StdioRuntimeDriver> {
    const binary = resolveRuntimeBinary(options);
    const clock = options.clock ?? "virtual";
    const args = ["--clock", clock];
    if (options.headless) {
      args.push("--headless");
    }
    if (options.stateDir !== undefined) {
      args.push("--state-dir", options.stateDir);
    }
    if (options.args) {
      args.push(...options.args);
    }

    const child = spawn(binary, args, { stdio: ["pipe", "pipe", "pipe"] });
    const driver = new StdioRuntimeDriver(child, clock);

    if (options.onStderr) {
      const onStderr = options.onStderr;
      createInterface({ input: child.stderr }).on("line", onStderr);
    } else {
      child.stderr.resume();
    }

    const hello = await driver.request<HelloResult>("hello");
    if (hello.protocol !== PROTOCOL_VERSION) {
      await driver.dispose();
      throw new Error(
        `Protocol mismatch: driver speaks v${PROTOCOL_VERSION}, sidecar speaks v${hello.protocol}.`,
      );
    }
    driver.luau = hello.luau;
    return driver;
  }

  #attach(): void {
    this.#child.on("error", (error) => {
      this.#failAll(new Error(`Failed to start the MicroStudio runtime: ${error.message}`));
    });
    this.#child.on("exit", (code, signal) => {
      const detail = signal ? `signal ${signal}` : `code ${code}`;
      this.#failAll(
        new Error(`The MicroStudio runtime exited unexpectedly (${detail}).`),
      );
    });

    const lines = createInterface({ input: this.#child.stdout });
    lines.on("line", (line) => {
      if (line.trim().length === 0) {
        return;
      }
      let message: RuntimeResponse & RuntimeNotification;
      try {
        message = JSON.parse(line) as RuntimeResponse & RuntimeNotification;
      } catch (error) {
        this.#failAll(
          new Error(`The MicroStudio runtime sent malformed JSON: ${line}`),
        );
        return;
      }
      this.#dispatch(message);
    });
  }

  #dispatch(message: RuntimeResponse & RuntimeNotification): void {
    if (typeof message.id !== "number") {
      for (const handler of this.#handlers) {
        handler({
          method: message.method,
          ...(message.params === undefined ? {} : { params: message.params }),
        });
      }
      return;
    }

    const pending = this.#pending.get(message.id);
    if (!pending) {
      return;
    }
    this.#pending.delete(message.id);

    if (message.ok) {
      pending.resolve(message.result);
      return;
    }
    const info = message.error ?? { message: "unknown runtime error" };
    pending.reject(
      new RuntimeCommandError(
        pending.method,
        info.message,
        info.location,
        info.traceback,
      ),
    );
  }

  #failAll(error: Error): void {
    this.#exitError = error;
    for (const pending of this.#pending.values()) {
      pending.reject(error);
    }
    this.#pending.clear();
  }

  request<T>(method: string, params?: unknown): Promise<T> {
    if (this.#disposed) {
      return Promise.reject(new Error("The MicroStudio runtime has been disposed."));
    }
    if (this.#exitError) {
      return Promise.reject(this.#exitError);
    }

    const id = this.#nextId++;
    const payload = JSON.stringify({
      id,
      method,
      params: params ?? {},
    });

    return new Promise<T>((resolve, reject) => {
      this.#pending.set(id, {
        method,
        resolve: (value) => resolve(value as T),
        reject,
      });
      this.#child.stdin.write(`${payload}\n`, (error) => {
        if (error) {
          this.#pending.delete(id);
          reject(error);
        }
      });
    });
  }

  onNotification(handler: (notification: RuntimeNotification) => void): () => void {
    this.#handlers.add(handler);
    return () => {
      this.#handlers.delete(handler);
    };
  }

  async dispose(): Promise<void> {
    if (this.#disposed) {
      return;
    }
    this.#disposed = true;
    try {
      await this.request<unknown>("shutdown").catch(() => undefined);
    } finally {
      this.#handlers.clear();
      this.#child.stdin.end();
      this.#child.kill();
    }
  }
}
