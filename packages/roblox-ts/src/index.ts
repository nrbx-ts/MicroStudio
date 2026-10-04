// reads the existing roblox-ts + Rojo setup; never replaces the compiler

export {
  ALL_SERVICES,
  buildPlaceEntries,
  classifyScriptFile,
  DEFAULT_SERVICES,
  findProjectFile,
  initFileClass,
  isInitFileName,
  mountDirectories,
  rbxtsIncludeFolder,
  readRojoProject,
  readText,
  scriptInstanceName,
} from "./rojo.ts";
export type { BuildPlaceOptions, RojoNode, RojoProject } from "./rojo.ts";
export { parseJsonc } from "./jsonc.ts";
export {
  compileProject,
  findCompiler,
  findCompiledSpec,
  readTsConfig,
  testConfigFile,
  TEST_CONFIG_NAME,
  writeTestConfig,
  type CompileOptions,
  type CompileResult,
  type ProjectCompiler,
  type TestConfigOptions,
  type TsConfigInfo,
} from "./compile.ts";
