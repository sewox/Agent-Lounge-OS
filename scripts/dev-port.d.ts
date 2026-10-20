/** Types for the single-source `scripts/dev-port.mjs`. */
import type { createServer as CreateServer } from "node:net";

export const DEFAULT_DEV_PORT: string;
export const DEFAULT_DEV_HOST: string;

export class InvalidDevPortError extends Error {
  constructor(raw: unknown);
}

export class PortInUseError extends Error {
  readonly port: string;
  constructor(port: string | number);
}

export function resolveDevHost(
  env?: NodeJS.ProcessEnv | Record<string, string | undefined>,
): string;

export function resolveDevPort(
  env?: NodeJS.ProcessEnv | Record<string, string | undefined>,
): string;

export function assertDevPortFree(
  port: string | number,
  opts?: { createServer?: typeof CreateServer },
): Promise<void>;

export function canBindIpv6Loopback(opts?: {
  createServer?: typeof CreateServer;
}): Promise<boolean>;

export function buildNextDevArgs(port: string, extraArgs?: string[]): string[];
