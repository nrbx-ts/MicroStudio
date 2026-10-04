// the Luau half: assertions live in the spec, the runner is TypeScript

export {
  DEFAULT_INCLUDE,
  DEFAULT_EXCLUDE,
  DEFAULT_TS_INCLUDE,
  discoverSpecs,
  globToRegExp,
  matchesPattern,
  type DiscoverOptions,
} from "./discover.ts";
export {
  DEFAULT_TIMEOUT_MS,
  FRAMEWORK_CHUNK,
  TestTimeoutError,
  formatFailure,
  framesOf,
  luaTest,
  moduleEntries,
  runSpecFiles,
  runSuite,
  summarize,
  type LuaTestOptions,
  type RunSuiteOptions,
  type SpecEntry,
} from "./runner.ts";
export {
  failedResults,
  formatSummary,
  formatSuite,
  formatTest,
  type ReporterOptions,
} from "./reporter.ts";
export { renderJsonReport } from "./json.ts";
export type {
  HookFailure,
  SuiteResult,
  SuiteSummary,
  TestMode,
  TestResult,
  TestStatus,
} from "./types.ts";
