#!/usr/bin/env node
// A prebuilt sidecar that needs a library the machine does not have is a release
// that installs and then fails to start, which is the one thing shipping a binary
// is meant to avoid. Each platform can be asked what a binary loads; this refuses
// anything the operating system does not ship itself.
//
// USAGE:
//   check-binary <path>

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const USAGE = `\
check-binary -- check that a built sidecar runs with nothing installed

USAGE:
  check-binary <path>

Reads the binary's own dependency table and fails, naming the library, if it
loads something a machine would have to install first: the visual c++
redistributable on windows, or anything past the c library on linux and macOS.
`;

// windows: these come from the visual c++ redistributable, which is not part of
// windows. every other dll an import table can name is one the system ships
const REDISTRIBUTABLE = /^(?:vcruntime|msvcp|msvcr|concrt|vccorlib)\w*\.dll$/i;

// linux: the c library and what gcc puts beside it, and nothing else
const SYSTEM_LIBRARIES =
  /^(?:libc|libm|libgcc_s|libpthread|libdl|librt|libutil|ld-linux)\b/;

interface PeHeaders {
  data: Buffer;
  sections: number;
  sectionTable: number;
  importRva: number;
}

// the header fields an import table lives behind
function peHeaders(binary: string): PeHeaders {
  const data = readFileSync(binary);
  const peOffset = data.readUInt32LE(0x3c);
  if (data.toString("latin1", peOffset, peOffset + 4) !== "PE\0\0") {
    throw new Error(`${binary} has no PE header.`);
  }

  const coff = peOffset + 4;
  const optional = coff + 20;
  // the data directories start at a different offset in PE32 and PE32+
  const directories = optional + (data.readUInt16LE(optional) === 0x20b ? 112 : 96);

  return {
    data,
    sections: data.readUInt16LE(coff + 2),
    sectionTable: optional + data.readUInt16LE(coff + 16),
    importRva: data.readUInt32LE(directories + 8),
  };
}

// an rva is section-relative; reading the file needs the raw offset
function fileOffset(headers: PeHeaders, rva: number): number {
  for (let index = 0; index < headers.sections; index += 1) {
    const base = headers.sectionTable + index * 40;
    const virtualSize = headers.data.readUInt32LE(base + 8);
    const virtualAddress = headers.data.readUInt32LE(base + 12);
    const rawSize = Math.max(virtualSize, headers.data.readUInt32LE(base + 16));
    if (rva >= virtualAddress && rva < virtualAddress + rawSize) {
      return headers.data.readUInt32LE(base + 20) + (rva - virtualAddress);
    }
  }
  throw new Error(`No section of the file holds the RVA ${rva}.`);
}

function peString(headers: PeHeaders, rva: number): string {
  const start = fileOffset(headers, rva);
  let end = start;
  while (end < headers.data.length && headers.data[end] !== 0) {
    end += 1;
  }
  return headers.data.toString("latin1", start, end);
}

function windowsDependencies(binary: string): string[] {
  const headers = peHeaders(binary);
  if (headers.importRva === 0) {
    return [];
  }

  const names: string[] = [];
  // one 20 byte descriptor per dll, and a zeroed one to end the table
  for (
    let descriptor = fileOffset(headers, headers.importRva);
    descriptor + 20 <= headers.data.length;
    descriptor += 20
  ) {
    const nameRva = headers.data.readUInt32LE(descriptor + 12);
    if (nameRva === 0) {
      break;
    }
    names.push(peString(headers, nameRva));
  }
  return names;
}

function tool(command: string, args: readonly string[]): string {
  return execFileSync(command, [...args], { encoding: "utf8" });
}

function linuxDependencies(binary: string): string[] {
  const needs = tool("readelf", ["-d", binary]).matchAll(
    /Shared library: \[([^\]]+)\]/g,
  );
  return [...needs].map((match) => match[1] ?? "");
}

function darwinDependencies(binary: string): string[] {
  return tool("otool", ["-L", binary])
    .split("\n")
    .filter((line) => line.includes("(compatibility version"))
    .map((line) => line.trim().split(" ")[0] ?? "");
}

export function readDependencies(
  binary: string,
  platform: string = process.platform,
): string[] {
  if (platform === "win32") {
    return windowsDependencies(binary);
  }
  if (platform === "darwin") {
    return darwinDependencies(binary);
  }
  return linuxDependencies(binary);
}

// what a machine would have to install before the binary would start
export function unexpectedDependencies(
  names: readonly string[],
  platform: string = process.platform,
): string[] {
  if (platform === "win32") {
    return names.filter((name) => REDISTRIBUTABLE.test(name));
  }
  if (platform === "darwin") {
    return names.filter(
      (name) => !name.startsWith("/usr/lib/") && !name.startsWith("/System/Library/"),
    );
  }
  return names.filter((name) => !SYSTEM_LIBRARIES.test(name.replace(/^.*\//, "")));
}

function main(argv: readonly string[]): number {
  const [binary] = argv;
  if (binary === undefined || binary === "--help") {
    process.stdout.write(USAGE);
    return binary === "--help" ? 0 : 2;
  }

  const names = readDependencies(binary);
  const unexpected = unexpectedDependencies(names);
  if (unexpected.length > 0) {
    console.error(
      [
        `${binary} loads something a machine may not have:`,
        ...unexpected.map((name) => `  ${name}`),
        "",
        "It ships as a prebuilt binary, so it has to run on a machine with nothing",
        "installed but node.",
      ].join("\n"),
    );
    return 1;
  }

  console.log(`${binary}: ${names.length} dependencies, all of them the system's own.`);
  return 0;
}

// importable, so the rules can be tested without a binary of every platform
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    process.exitCode = main(process.argv.slice(2));
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
