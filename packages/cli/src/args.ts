import { resolve } from "node:path";

export interface ParsedArgs {
  // subcommand, or first positional when there is none
  command: string;
  positionals: string[];
  flags: Map<string, string | boolean>;
}

const FLAGS_WITH_VALUES = new Set([
  "--project",
  "--services",
  "--include",
  "--exclude",
  "--module",
  "--clock",
  "--timeout",
  "--eval",
  "--tsconfig",
  "--state-dir",
  "-e",
]);

// hand-rolled parser: no runtime dependencies
// first token is a command only when it is not a flag; a bare `-` is not a flag
export function parseArgs(argv: readonly string[]): ParsedArgs {
  const [first, ...rest] = argv;
  const hasCommand = first !== undefined && !first.startsWith("-");
  const command = hasCommand ? first : "";
  const tokens = hasCommand ? rest : argv;

  const positionals: string[] = [];
  const flags = new Map<string, string | boolean>();

  for (let index = 0; index < tokens.length; index += 1) {
    const arg = tokens[index];
    if (arg === undefined) {
      continue;
    }
    if (arg.startsWith("-") && arg !== "-") {
      const [name, inline] = arg.split("=", 2);
      const key = name ?? arg;
      if (inline !== undefined) {
        flags.set(key, inline);
        continue;
      }
      if (FLAGS_WITH_VALUES.has(key)) {
        const value = tokens[index + 1];
        if (value !== undefined && !value.startsWith("-")) {
          flags.set(key, value);
          index += 1;
          continue;
        }
      }
      flags.set(key, true);
      continue;
    }
    positionals.push(arg);
  }

  return { command, positionals, flags };
}

export function flagString(args: ParsedArgs, name: string): string | undefined {
  const value = args.flags.get(name);
  return typeof value === "string" ? value : undefined;
}

export function flagBool(args: ParsedArgs, name: string): boolean {
  return args.flags.get(name) === true;
}

export function projectDir(args: ParsedArgs): string {
  const positional = args.positionals[0];
  return resolve(positional ?? process.cwd());
}
