/**
 * Poll until predicate returns a truthy value (event-based wait; no fixed sleep).
 * @param {() => unknown} predicate
 * @param {{ timeoutMs?: number, intervalMs?: number, label?: string }} [opts]
 */
export async function waitFor(predicate, opts = {}) {
  const timeoutMs = opts.timeoutMs ?? 10_000;
  const intervalMs = opts.intervalMs ?? 25;
  const label = opts.label || "condition";
  const start = Date.now();
  for (;;) {
    let value;
    try {
      value = predicate();
      if (value) return value;
    } catch {
      /* retry */
    }
    if (Date.now() - start > timeoutMs) {
      throw new Error(`waitFor timeout after ${timeoutMs}ms (${label})`);
    }
    await new Promise((r) => setTimeout(r, intervalMs));
  }
}

/** Generous default for CI windows/macos scheduling jitter. */
export const CI_WAIT_MS = 15_000;
