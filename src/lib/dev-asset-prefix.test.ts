import assert from "node:assert/strict";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { describe, it } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import {
  assertDevPortFree,
  buildNextDevArgs,
  canBindIpv6Loopback,
  DEFAULT_DEV_HOST,
  DEFAULT_DEV_PORT,
  InvalidDevPortError,
  PortInUseError,
  resolveDevAssetPrefix,
  resolveDevPort,
} from "./dev-asset-prefix.ts";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const devPortMjs = path.join(root, "scripts/dev-port.mjs");
const nextDevMjs = path.join(root, "scripts/next-dev.mjs");

/** Bind with an explicit host (e.g. 127.0.0.1 / ::1). */
function listenOnHost(port: number, host: string): Promise<net.Server> {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once("error", reject);
    server.listen(port, host, () => resolve(server));
  });
}

/** Bind with no host — same as Next / assertDevPortFree (every address). */
function listenAllAddresses(port: number): Promise<net.Server> {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once("error", reject);
    server.listen(port, () => resolve(server));
  });
}

function closeServer(server: net.Server): Promise<void> {
  return new Promise((resolve, reject) => {
    server.close((err) => (err ? reject(err) : resolve()));
  });
}

function assertPortInUseError(err: unknown, port: number): boolean {
  assert.ok(err instanceof PortInUseError);
  assert.match((err as Error).message, new RegExp(String(port)));
  assert.match((err as Error).message, /already in use/i);
  assert.match((err as Error).message, /PORT=/);
  return true;
}

describe("dev assetPrefix / PORT — single source", () => {
  it("defaults to DEFAULT_DEV_PORT when PORT is unset or empty/whitespace", () => {
    assert.equal(DEFAULT_DEV_PORT, "3000");
    assert.equal(resolveDevPort({}), DEFAULT_DEV_PORT);
    assert.equal(resolveDevPort({ PORT: "" }), DEFAULT_DEV_PORT);
    assert.equal(resolveDevPort({ PORT: "   " }), DEFAULT_DEV_PORT);
  });

  it("follows PORT for the non-prod assetPrefix", () => {
    assert.equal(
      resolveDevAssetPrefix({
        isProd: false,
        host: "localhost",
        port: resolveDevPort({ PORT: "4123" }),
      }),
      "http://localhost:4123",
    );
    assert.equal(
      resolveDevAssetPrefix({
        isProd: false,
        host: "127.0.0.1",
        port: resolveDevPort({ PORT: "3001" }),
      }),
      "http://127.0.0.1:3001",
    );
  });

  it("uses DEFAULT_DEV_PORT / DEFAULT_DEV_HOST when assetPrefix opts omit them", () => {
    assert.equal(
      resolveDevAssetPrefix({ isProd: false }),
      `http://${DEFAULT_DEV_HOST}:${DEFAULT_DEV_PORT}`,
    );
    const src = fs.readFileSync(
      path.join(root, "src/lib/dev-asset-prefix.ts"),
      "utf8",
    );
    assert.match(src, /DEFAULT_DEV_PORT/);
    assert.match(src, /DEFAULT_DEV_HOST/);
    assert.doesNotMatch(src, /\|\|\s*["']3000["']/);
  });

  it("omits assetPrefix in production", () => {
    assert.equal(
      resolveDevAssetPrefix({ isProd: true, port: "4123" }),
      undefined,
    );
  });

  it("src/lib re-exports resolveDevPort from scripts/dev-port.mjs only", () => {
    const ts = fs.readFileSync(
      path.join(root, "src/lib/dev-asset-prefix.ts"),
      "utf8",
    );
    assert.match(ts, /from\s+["']\.\.\/\.\.\/scripts\/dev-port\.mjs["']/);
    assert.doesNotMatch(ts, /export function resolveDevPort/);
    const nextDev = fs.readFileSync(nextDevMjs, "utf8");
    assert.match(nextDev, /from\s+["']\.\/dev-port\.mjs["']/);
    assert.doesNotMatch(nextDev, /export function resolveDevPort/);
  });

  it("scripts/dev-port.mjs is the same resolveDevPort the TS re-export uses", () => {
    const moduleUrl = pathToFileURL(devPortMjs).href;
    const probe = `
      import { resolveDevPort } from ${JSON.stringify(moduleUrl)};
      const port = resolveDevPort({ PORT: process.env.PROBE_PORT });
      process.stdout.write(port);
    `;
    const result = spawnSync(
      process.execPath,
      ["--input-type=module", "-e", probe],
      {
        encoding: "utf8",
        env: { ...process.env, PROBE_PORT: "4455" },
      },
    );
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "4455");
    assert.equal(resolveDevPort({ PORT: "4455" }), "4455");
  });

  it("next.config.ts wires resolveDevAssetPrefix to PORT", () => {
    const conf = fs.readFileSync(path.join(root, "next.config.ts"), "utf8");
    assert.match(conf, /resolveDevAssetPrefix/);
    assert.match(conf, /resolveDevPort/);
    assert.match(conf, /from\s+["'].*dev-asset-prefix["']/);
  });

  it("package.json dev script pins next via next-dev.mjs", () => {
    const pkg = JSON.parse(
      fs.readFileSync(path.join(root, "package.json"), "utf8"),
    );
    assert.equal(pkg.scripts.dev, "node scripts/next-dev.mjs");
  });
});

describe("dev PORT validation", () => {
  it("accepts integers from 1 to 65535", () => {
    assert.equal(resolveDevPort({ PORT: "1" }), "1");
    assert.equal(resolveDevPort({ PORT: "65535" }), "65535");
    assert.equal(resolveDevPort({ PORT: "08080" }), "8080");
  });

  it("throws InvalidDevPortError with a clear message for non-integers", () => {
    for (const bad of ["abc", "3000.5", "-1", "+80", "0x50", "3e3"]) {
      assert.throws(
        () => resolveDevPort({ PORT: bad }),
        (err: unknown) => {
          assert.ok(err instanceof InvalidDevPortError);
          assert.match(
            (err as Error).message,
            /Invalid PORT .*must be an integer from 1 to 65535/,
          );
          return true;
        },
        bad,
      );
    }
  });

  it("throws for out-of-range integers 0 and 65536+", () => {
    for (const bad of ["0", "65536", "99999"]) {
      assert.throws(
        () => resolveDevPort({ PORT: bad }),
        (err: unknown) => {
          assert.ok(err instanceof InvalidDevPortError);
          assert.match((err as Error).message, /Invalid PORT/);
          return true;
        },
        bad,
      );
    }
  });

  it("next-dev.mjs exits non-zero and prints the message on invalid PORT", () => {
    const result = spawnSync(process.execPath, [nextDevMjs], {
      encoding: "utf8",
      env: { ...process.env, PORT: "not-a-port" },
    });
    assert.notEqual(result.status, 0);
    assert.match(
      `${result.stdout}${result.stderr}`,
      /Invalid PORT .*must be an integer from 1 to 65535/,
    );
  });
});

describe("next-dev CLI arg passthrough", () => {
  it("buildNextDevArgs pins -p and appends extra args", () => {
    assert.deepEqual(buildNextDevArgs("3000"), ["dev", "-p", "3000"]);
    assert.deepEqual(buildNextDevArgs("4123", ["--turbo"]), [
      "dev",
      "-p",
      "4123",
      "--turbo",
    ]);
    assert.deepEqual(buildNextDevArgs("3001", ["--hostname", "0.0.0.0"]), [
      "dev",
      "-p",
      "3001",
      "--hostname",
      "0.0.0.0",
    ]);
  });

  it("next-dev.mjs forwards process.argv extras via buildNextDevArgs", () => {
    const src = fs.readFileSync(nextDevMjs, "utf8");
    assert.match(src, /buildNextDevArgs/);
    assert.match(src, /process\.argv\.slice\(2\)/);
    assert.match(src, /\[nextBin,\s*\.\.\.nextArgs\]/);
  });
});

describe("busy port pre-flight", () => {
  it("assertDevPortFree rejects when the port is held on all addresses", async () => {
    const holder = await listenAllAddresses(0);
    const addr = holder.address();
    assert.ok(addr && typeof addr === "object");
    const port = addr.port;
    try {
      await assert.rejects(
        () => assertDevPortFree(port),
        (err: unknown) => assertPortInUseError(err, port),
      );
    } finally {
      await closeServer(holder);
    }
  });

  it("assertDevPortFree rejects when the holder is only on 127.0.0.1", async () => {
    const holder = await listenOnHost(0, "127.0.0.1");
    const addr = holder.address();
    assert.ok(addr && typeof addr === "object");
    const port = addr.port;
    try {
      await assert.rejects(
        () => assertDevPortFree(port),
        (err: unknown) => assertPortInUseError(err, port),
      );
      const result = spawnSync(process.execPath, [nextDevMjs], {
        encoding: "utf8",
        env: { ...process.env, PORT: String(port) },
        timeout: 15_000,
      });
      assert.notEqual(result.status, 0, `${result.stdout}${result.stderr}`);
      const out = `${result.stdout}${result.stderr}`;
      assert.match(out, new RegExp(String(port)));
      assert.match(out, /already in use/i);
      assert.match(out, /PORT=/);
    } finally {
      await closeServer(holder);
    }
  });

  it("assertDevPortFree rejects ::1-only holders when IPv6 loopback is bindable", async () => {
    const ipv6Ok = await canBindIpv6Loopback();
    if (!ipv6Ok) {
      // Deterministic: assert unavailability, then cover via IPv4-only holder.
      assert.equal(ipv6Ok, false);
      const holder = await listenOnHost(0, "127.0.0.1");
      const addr = holder.address();
      assert.ok(addr && typeof addr === "object");
      const port = addr.port;
      try {
        await assert.rejects(
          () => assertDevPortFree(port),
          (err: unknown) => assertPortInUseError(err, port),
        );
      } finally {
        await closeServer(holder);
      }
      return;
    }

    const holder = await listenOnHost(0, "::1");
    const addr = holder.address();
    assert.ok(addr && typeof addr === "object");
    const port = addr.port;
    try {
      await assert.rejects(
        () => assertDevPortFree(port),
        (err: unknown) => assertPortInUseError(err, port),
      );
      const result = spawnSync(process.execPath, [nextDevMjs], {
        encoding: "utf8",
        env: { ...process.env, PORT: String(port) },
        timeout: 15_000,
      });
      assert.notEqual(result.status, 0, `${result.stdout}${result.stderr}`);
      const out = `${result.stdout}${result.stderr}`;
      assert.match(out, new RegExp(String(port)));
      assert.match(out, /already in use/i);
      assert.match(out, /PORT=/);
    } finally {
      await closeServer(holder);
    }
  });

  it("assertDevPortFree resolves on port 0 (ephemeral free bind, no race)", async () => {
    // Port 0 asks the OS for a free port inside the probe itself — no
    // learn-then-probe gap. Also proves createServer injection works.
    await assertDevPortFree(0);
    await assertDevPortFree(0, { createServer: net.createServer });
  });

  it("next-dev.mjs exits non-zero when the port is held on all addresses", async () => {
    const holder = await listenAllAddresses(0);
    const addr = holder.address();
    assert.ok(addr && typeof addr === "object");
    const port = addr.port;
    try {
      const result = spawnSync(process.execPath, [nextDevMjs], {
        encoding: "utf8",
        env: { ...process.env, PORT: String(port) },
        timeout: 15_000,
      });
      assert.notEqual(
        result.status,
        0,
        `expected non-zero exit; stdout=${result.stdout} stderr=${result.stderr}`,
      );
      const out = `${result.stdout}${result.stderr}`;
      assert.match(out, new RegExp(String(port)));
      assert.match(out, /already in use/i);
      assert.match(out, /PORT=/);
      assert.notEqual(result.signal, "SIGTERM");
    } finally {
      await closeServer(holder);
    }
  });

  it("next-dev.mjs does not pass -H (Next keeps default all-address bind)", () => {
    const src = fs.readFileSync(nextDevMjs, "utf8");
    assert.doesNotMatch(src, /["']-H["']|["']--hostname["']/);
    assert.match(src, /assertDevPortFree\(port\)/);
  });
});
