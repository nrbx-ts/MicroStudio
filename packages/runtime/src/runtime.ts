import type { RuntimeDriver } from "./driver.ts";
import type {
  EvalMode,
  InstanceJson,
  LoadTreeResult,
  LogNotification,
  PlaceEntry,
  RunScriptsResult,
  ScriptErrorNotification,
  StatsResult,
  TreeResult,
} from "./protocol.ts";
import { StdioRuntimeDriver, type StdioDriverOptions } from "./stdio-driver.ts";

export interface RuntimeOptions extends StdioDriverOptions {
  // inject a driver instead of spawning the sidecar
  driver?: RuntimeDriver;
}

export class PlayersApi {
  #runtime: Runtime;

  constructor(runtime: Runtime) {
    this.#runtime = runtime;
  }

  // fires PlayerAdded before resolving; earlier listeners see the join
  async addMockPlayer(
    name: string,
    options: { withCharacter?: boolean; userId?: number } = {},
  ): Promise<InstanceJson> {
    const result = await this.#runtime.driver.request<{ player: InstanceJson }>(
      "addMockPlayer",
      {
        name,
        withCharacter: options.withCharacter ?? false,
        userId: options.userId ?? null,
      },
    );
    return result.player;
  }
}

export class Runtime {
  readonly driver: RuntimeDriver;
  readonly players: PlayersApi;
  readonly clock: "virtual" | "real";

  get luauVersion(): string {
    return this.driver.luau;
  }

  #logs: LogNotification[] = [];
  #errors: ScriptErrorNotification[] = [];
  #outputHandlers = new Set<(log: LogNotification) => void>();
  #errorHandlers = new Set<(error: ScriptErrorNotification) => void>();
  #unsubscribe: () => void;
  #disposed = false;

  constructor(driver: RuntimeDriver) {
    this.driver = driver;
    this.clock = driver.clock;
    this.players = new PlayersApi(this);
    this.#unsubscribe = driver.onNotification((notification) => {
      this.#handle(notification.method, notification.params ?? {});
    });
  }

  #handle(method: string, params: Record<string, unknown>): void {
    if (method === "log") {
      const log: LogNotification = {
        level: (params["level"] as LogNotification["level"]) ?? "print",
        text: String(params["text"] ?? ""),
        source: (params["source"] as string | null | undefined) ?? null,
      };
      this.#logs.push(log);
      for (const handler of this.#outputHandlers) {
        handler(log);
      }
      return;
    }

    if (method === "warn") {
      const log: LogNotification = {
        level: "warn",
        text: String(params["message"] ?? ""),
        source: null,
      };
      this.#logs.push(log);
      for (const handler of this.#outputHandlers) {
        handler(log);
      }
      return;
    }

    if (method === "error") {
      const error: ScriptErrorNotification = {
        location: String(params["location"] ?? "unknown"),
        message: String(params["message"] ?? ""),
        traceback: String(params["traceback"] ?? ""),
        rendered: String(params["rendered"] ?? ""),
      };
      this.#errors.push(error);
      for (const handler of this.#errorHandlers) {
        handler(error);
      }
    }
  }

  async eval(
    code: string,
    options: { mode?: EvalMode; name?: string } = {},
  ): Promise<unknown> {
    const result = await this.driver.request<{ value: unknown }>("eval", {
      code,
      ...(options.mode === undefined ? {} : { mode: options.mode }),
      ...(options.name === undefined ? {} : { name: options.name }),
    });
    return result.value;
  }

  // standalone scripts must finish scheduled work before exit
  async drain(): Promise<number> {
    const result = await this.driver.request<{ time: number }>("drain");
    return result.time;
  }

  // resets instances/signals/threads/require cache; lua registry survives
  async resetWorld(): Promise<number> {
    const result = await this.driver.request<{ time: number }>("resetWorld");
    return result.time;
  }

  // runs due work without sleeping; real-clock sessions pump on an interval
  async pump(): Promise<number> {
    const result = await this.driver.request<{ time: number }>("pump");
    return result.time;
  }

  async advanceTime(seconds: number): Promise<number> {
    const result = await this.driver.request<{ time: number }>("advanceTime", {
      seconds,
    });
    return result.time;
  }

  async run(): Promise<string[]> {
    const result = await this.driver.request<RunScriptsResult>("runScripts", {});
    return result.scripts;
  }

  async runScript(path: string): Promise<string[]> {
    const result = await this.driver.request<RunScriptsResult>("runScripts", {
      path,
    });
    return result.scripts;
  }

  async runSource(
    name: string,
    source: string,
    parent?: string,
  ): Promise<string[]> {
    const result = await this.driver.request<RunScriptsResult>("runScripts", {
      source,
      name,
      parent: parent ?? null,
    });
    return result.scripts;
  }

  async loadTree(entries: PlaceEntry[]): Promise<string[]> {
    const result = await this.driver.request<LoadTreeResult>("loadTree", {
      entries,
    });
    return result.created;
  }

  async tree(path?: string, depth = 4): Promise<InstanceJson> {
    const result = await this.driver.request<TreeResult>("getTree", {
      path: path ?? null,
      depth,
    });
    return result.root;
  }

  async inspect(path: string): Promise<InstanceJson> {
    const result = await this.driver.request<{ instance: InstanceJson }>("inspect", {
      path,
    });
    return result.instance;
  }

  async stats(): Promise<StatsResult> {
    return this.driver.request<StatsResult>("stats");
  }

  async listServices(): Promise<string[]> {
    const result = await this.driver.request<{ services: string[] }>(
      "listServices",
    );
    return result.services;
  }

  logs(): readonly LogNotification[] {
    return this.#logs;
  }

  errors(): readonly ScriptErrorNotification[] {
    return this.#errors;
  }

  clearOutput(): void {
    this.#logs = [];
    this.#errors = [];
  }

  onOutput(handler: (log: LogNotification) => void): () => void {
    this.#outputHandlers.add(handler);
    return () => {
      this.#outputHandlers.delete(handler);
    };
  }

  onError(handler: (error: ScriptErrorNotification) => void): () => void {
    this.#errorHandlers.add(handler);
    return () => {
      this.#errorHandlers.delete(handler);
    };
  }

  async dispose(): Promise<void> {
    if (this.#disposed) {
      return;
    }
    this.#disposed = true;
    this.#unsubscribe();
    this.#outputHandlers.clear();
    this.#errorHandlers.clear();
    await this.driver.dispose();
  }
}

// defaults: virtual clock, no Studio semantics; dev passes clock: "real"
export async function createRuntime(options: RuntimeOptions = {}): Promise<Runtime> {
  const driver = options.driver ?? (await StdioRuntimeDriver.start(options));
  return new Runtime(driver);
}
