declare module "bun:test" {
  export function describe(label: string, fn: () => void): void;
  export function it(
    label: string,
    fn: () => void | Promise<void>
  ): void;
  export function expect<T>(actual: T): {
    toBe(expected: T): void;
    toEqual(expected: unknown): void;
    toBeNull(): void;
  };
}
