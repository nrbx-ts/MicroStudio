import { createRuntime, type Runtime, type RuntimeOptions } from "@microstudio/runtime";

// adds lifetime management: a failing assertion cannot leak a sidecar process

// virtual clock by default
export async function createTestRuntime(
  options: RuntimeOptions = {},
): Promise<Runtime> {
  return createRuntime({ clock: "virtual", ...options });
}

// disposes the runtime even when body throws
export async function withRuntime<T>(
  body: (runtime: Runtime) => Promise<T> | T,
  options: RuntimeOptions = {},
): Promise<T> {
  const runtime = await createTestRuntime(options);
  try {
    return await body(runtime);
  } finally {
    await runtime.dispose();
  }
}

// re-exported so tests need one import
export { createRuntime } from "@microstudio/runtime";
export type {
  InstanceJson,
  LogNotification,
  Runtime,
  RuntimeOptions,
} from "@microstudio/runtime";

// spec files, the `microstudio test` runner, and `luaTest`
export * from "./framework/index.ts";

// reads a `number` out of an eval result, throwing when it is not one
export function asNumber(value: unknown): number {
  if (typeof value !== "number") {
    throw new Error(`Expected a number from eval, got ${JSON.stringify(value)}`);
  }
  return value;
}

// reads a `string` out of an eval result
export function asString(value: unknown): string {
  if (typeof value !== "string") {
    throw new Error(`Expected a string from eval, got ${JSON.stringify(value)}`);
  }
  return value;
}

// reads an array out of an eval result
export function asArray<T = unknown>(value: unknown): T[] {
  if (!Array.isArray(value)) {
    throw new Error(`Expected an array from eval, got ${JSON.stringify(value)}`);
  }
  return value as T[];
}
