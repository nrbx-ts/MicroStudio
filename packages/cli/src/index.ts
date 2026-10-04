#!/usr/bin/env node
// no build step: node runs the .ts directly

import { flagBool, parseArgs } from "./args.ts";
import { dev, help, repl, standalone, test, VERSION } from "./commands.ts";

const USAGE = `\
microstudio — a local, headless Roblox development runtime

Usage: microstudio [<file.luau>] [-e <code>]        run a script, or prompt
       microstudio <dev|test|repl|run> [options]`;

async function main(argv: readonly string[]): Promise<number> {
  const args = parseArgs(argv);

  if (args.command === "help" || flagBool(args, "--help") || flagBool(args, "-h")) {
    return help();
  }
  if (args.command === "version" || flagBool(args, "--version") || flagBool(args, "-v")) {
    console.log(`microstudio ${VERSION}`);
    return 0;
  }

  switch (args.command) {
    case "dev":
      return dev(args);
    case "test":
      return test(args);
    case "repl":
      return repl(args);
    // `run` and a bare invocation land here, prompting when given nothing
    case "run":
    case "":
      return standalone(args, "");
    default:
      // no subcommand: the first word is a path
      return standalone(args, args.command);
  }
}

process.exitCode = await main(process.argv.slice(2));
