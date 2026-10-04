// a suite is one spec file: one runtime, one world; a test is one `it` block

export type TestStatus = "passed" | "failed" | "skipped" | "todo";

// declared mode, before `.only` filtering
export type TestMode = TestStatus | "run" | "only";

export interface HookFailure {
  hook: "beforeAll" | "afterAll" | "beforeEach" | "afterEach";
  location?: string | null;
  message: string;
  traceback?: string | null;
}

export interface TestResult {
  // `Suite > nested suite > test`, the name a failure is reported under
  name: string;
  shortName: string;
  file: string;
  // nesting depth, for reporter indentation
  depth: number;
  status: TestStatus;
  durationMs: number;
  assertions: number;
  message?: string;
  traceback?: string;
  hookFailures?: HookFailure[];
  logs: string[];
}

export interface SuiteResult {
  file: string;
  tests: TestResult[];
  // set when the spec could not load (syntax error or throw)
  loadError?: string;
  durationMs: number;
  logs: string[];
}

export interface SuiteSummary {
  files: number;
  loadFailures: number;
  // files with a failing test, or that did not load
  failedFiles: number;
  tests: number;
  passed: number;
  failed: number;
  skipped: number;
  todo: number;
  assertions: number;
  durationMs: number;
  ok: boolean;
}
