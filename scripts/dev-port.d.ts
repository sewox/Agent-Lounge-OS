/** Types for the single-source `scripts/dev-port.mjs`. */

export const DEFAULT_DEV_PORT: string;
export const DEFAULT_DEV_HOST: string;

export class InvalidDevPortError extends Error {
  constructor(raw: unknown);
}

export class PortInUseError extends Error {
  readonly port: string;
  readonly host: string;
  constructor(port: string | number, host: string);
}

export function resolveDevHost(
  env?: NodeJS.ProcessEnv | Record<string, string | undefined>,
): string;

export function resolveDevPort(
  env?: NodeJS.ProcessEnv | Record<string, string | undefined>,
): string;

export function assertDevPortFree(
  port: string | number,
  host?: string,
): Promise<void>;

export function buildNextDevArgs(port: string, extraArgs?: string[]): string[];
