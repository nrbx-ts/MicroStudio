// results plus summary, with no formatting decisions baked in

import type { SuiteResult, SuiteSummary } from "./types.ts";

export interface JsonReport {
  summary: SuiteSummary;
  files: SuiteResult[];
}

export function renderJsonReport(
  results: readonly SuiteResult[],
  summary: SuiteSummary,
): string {
  const report: JsonReport = { summary, files: [...results] };
  return JSON.stringify(report, null, 2);
}
