// no I/O: formatters return lines, callers write them

import type {
  SuiteResult,
  SuiteSummary,
  TestResult,
  TestStatus,
} from "./types.ts";

const SYMBOLS: Record<TestStatus, string> = {
  passed: "✓",
  failed: "✗",
  skipped: "⊘",
  todo: "☐",
};

const COLOURS: Record<TestStatus, string> = {
  passed: "\u001b[32m",
  failed: "\u001b[31m",
  skipped: "\u001b[33m",
  todo: "\u001b[36m",
};

const RESET = "\u001b[0m";
const DIM = "\u001b[2m";

export interface ReporterOptions {
  // defaults to stdout being a TTY
  color?: boolean;
  // defaults to true, as jest does
  verbose?: boolean;
  // defaults to only on failure
  showLogs?: boolean;
}

function painter(options: ReporterOptions): {
  colour: (text: string, code: string) => string;
  dim: (text: string) => string;
} {
  const enabled = options.color ?? process.stdout.isTTY === true;
  return {
    colour: (text, code) => (enabled ? `${code}${text}${RESET}` : text),
    dim: (text) => (enabled ? `${DIM}${text}${RESET}` : text),
  };
}

const INDENT = "  ";

export function formatTest(
  test: TestResult,
  options: ReporterOptions = {},
): string[] {
  const { colour, dim } = painter(options);
  const indent = INDENT.repeat(test.depth + 1);
  const lines: string[] = [];

  if (test.status === "passed" && options.verbose === false) {
    return lines;
  }

  const symbol = colour(SYMBOLS[test.status], COLOURS[test.status]);
  const duration =
    test.status === "passed" || test.status === "failed"
      ? dim(` ${test.durationMs}ms`)
      : "";
  lines.push(`${indent}${symbol} ${test.shortName}${duration}`);

  if (test.status === "failed") {
    const detail = indent + INDENT;
    if (test.message !== undefined) {
      lines.push(colour(`${detail}${test.message}`, COLOURS.failed));
    }
    if (test.traceback !== undefined) {
      const frames = test.traceback.split("\n").filter((frame) => frame.length > 0);
      // one frame is the failure's own location, already in the message
      if (frames.length > 1) {
        for (const frame of frames) {
          lines.push(dim(`${detail}at ${frame}`));
        }
      }
    }
    for (const log of test.logs) {
      lines.push(dim(`${detail}${log}`));
    }
  } else if (options.showLogs === true) {
    for (const log of test.logs) {
      lines.push(dim(`${INDENT.repeat(test.depth + 2)}${log}`));
    }
  }

  return lines;
}

export function formatSuite(
  result: SuiteResult,
  options: ReporterOptions = {},
): string[] {
  const { colour, dim } = painter(options);
  const lines: string[] = [];
  const failed = result.tests.filter((test) => test.status === "failed").length;
  const symbol = colour(SYMBOLS[failed > 0 ? "failed" : "passed"], COLOURS[failed > 0 ? "failed" : "passed"]);

  lines.push(
    `${symbol} ${result.file} ${dim(
      `(${result.tests.length} test${result.tests.length === 1 ? "" : "s"}, ${result.durationMs}ms)`,
    )}`,
  );

  if (result.loadError !== undefined) {
    lines.push(colour(`${INDENT}${result.loadError}`, COLOURS.failed));
    return lines;
  }

  for (const test of result.tests) {
    lines.push(...formatTest(test, options));
  }
  for (const log of result.logs) {
    lines.push(dim(`${INDENT}${log}`));
  }

  return lines;
}

export function formatSummary(
  summary: SuiteSummary,
  options: ReporterOptions = {},
): string[] {
  const { colour, dim } = painter(options);
  const label = (text: string): string => text.padStart(11);

  const files: string[] = [];
  if (summary.failedFiles > 0) {
    files.push(colour(`${summary.failedFiles} failed`, COLOURS.failed));
  }
  const passedFiles = summary.files - summary.failedFiles;
  if (passedFiles > 0) {
    files.push(colour(`${passedFiles} passed`, COLOURS.passed));
  }
  const fileCounts = files.length === 0 ? "no files" : files.join(" | ");

  const tests: string[] = [];
  if (summary.passed > 0) {
    tests.push(colour(`${summary.passed} passed`, COLOURS.passed));
  }
  if (summary.failed > 0) {
    tests.push(colour(`${summary.failed} failed`, COLOURS.failed));
  }
  if (summary.skipped > 0) {
    tests.push(colour(`${summary.skipped} skipped`, COLOURS.skipped));
  }
  if (summary.todo > 0) {
    tests.push(colour(`${summary.todo} todo`, COLOURS.todo));
  }

  return [
    `${label("Test files")}  ${fileCounts} (${summary.files})`,
    `${label("Tests")}  ${
      tests.length === 0 ? "no tests found" : `${tests.join(" | ")} (${summary.tests})`
    }`,
    `${label("Assertions")}  ${dim(String(summary.assertions))}`,
    `${label("Duration")}  ${dim(`${summary.durationMs}ms`)}`,
  ];
}

export function failedResults(results: readonly SuiteResult[]): TestResult[] {
  return results.flatMap((result) =>
    result.tests.filter((test) => test.status === "failed"),
  );
}
