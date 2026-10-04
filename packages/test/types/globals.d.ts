// declaration-only globals: microstudio injects them before a spec loads
// members are function properties, not methods: roblox-ts compiles a method call
// to a colon call, which would hand the matcher chain over as the expected value

interface MicroStudioMatchers<T> {
  // identity; reference equality for Roblox types
  toBe: (expected: T) => void;
  // structural equality, reports the first differing key
  toEqual: (expected: unknown) => void;
  toBeCloseTo: (expected: number, places?: number) => void;
  toBeTruthy: () => void;
  toBeFalsy: () => void;
  toBeNil: () => void;
  // the name Luau's typeof returns
  toBeType: (typeName: string) => void;
  // IsA, so inherited classes count
  toBeA: (className: string) => void;
  toHaveLength: (expected: number) => void;
  toContain: (value: unknown) => void;
  // a Luau pattern, or a plain substring
  toMatch: (pattern: string) => void;
  toThrow: (pattern?: string) => void;
  toHaveProperty: (name: string, expected?: unknown) => void;
  toBeGreaterThan: (expected: number) => void;
  toBeGreaterThanOrEqual: (expected: number) => void;
  toBeLessThan: (expected: number) => void;
  toBeLessThanOrEqual: (expected: number) => void;
  // `not` is a Luau keyword, so the negated chain is spelled `never`
  never: MicroStudioMatchers<T>;
  // alias for `never`, chai-style
  isNot: MicroStudioMatchers<T>;
}

interface MicroStudioTest {
  (name: string, body?: () => void): void;
  skip: (name: string, body?: () => void) => void;
  only: (name: string, body?: () => void) => void;
  todo: (name: string) => void;
}

interface MicroStudioSuite {
  (name: string, body: () => void): void;
  skip: (name: string, body: () => void) => void;
  only: (name: string, body: () => void) => void;
}

declare const describe: MicroStudioSuite;
declare const it: MicroStudioTest;
declare const test: MicroStudioTest;

declare function beforeAll(body: () => void): void;
declare function afterAll(body: () => void): void;
declare function beforeEach(body: () => void): void;
declare function afterEach(body: () => void): void;

declare function expect<T>(value: T, message?: string): MicroStudioMatchers<T>;

// replaces the world: no instances, players or timers
declare function reset(): void;

// PlayerAdded fires before this returns
declare function addPlayer(name?: string): Player;
