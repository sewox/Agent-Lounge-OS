/** Types for the single-source `scripts/dev-port.mjs`. */
export class InvalidDevPortError extends Error {
  constructor(raw: unknown);
}

export function resolveDevPort(
  env?: NodeJS.ProcessEnv | Record<string, string | undefined>,
): string;

export function buildNextDevArgs(port: string, extraArgs?: string[]): string[];
