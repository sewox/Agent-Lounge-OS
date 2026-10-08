import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { describe, it } from "node:test";

const script = path.resolve("scripts/ci/install-linux-build-deps.sh");

type OsClassify = {
  apt_based: boolean;
  id: string;
  id_like: string;
};

function classifyOsRelease(contents: string): OsClassify {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "os-release-"));
  const file = path.join(dir, "os-release");
  fs.writeFileSync(file, contents, "utf8");
  const result = spawnSync("bash", [script, "--classify-os-release", file], {
    encoding: "utf8",
    env: process.env,
  });
  fs.rmSync(dir, { recursive: true, force: true });
  assert.equal(
    result.status,
    0,
    `classify failed:\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
  );
  const line = result.stdout.trim().split("\n").at(-1) ?? "";
  const apt = /apt_based=(true|false)/.exec(line);
  const id = /(?:^|\s)id=([^\s]*)/.exec(line);
  const like = /(?:^|\s)id_like=(.*)$/.exec(line);
  assert.ok(apt, `missing apt_based in: ${line}`);
  assert.ok(id, `missing id in: ${line}`);
  return {
    apt_based: apt[1] === "true",
    id: id[1],
    id_like: like?.[1] ?? "",
  };
}

function hostIsAptFamily(): boolean {
  const release = "/etc/os-release";
  if (!fs.existsSync(release)) {
    return false;
  }
  const result = spawnSync("bash", [script, "--classify-os-release", release], {
    encoding: "utf8",
    env: process.env,
  });
  if (result.status !== 0) {
    return false;
  }
  return /apt_based=true/.test(result.stdout);
}

describe("install-linux-build-deps os-release detection", () => {
  it("classifies ubuntu as apt-based", () => {
    const c = classifyOsRelease(`ID=ubuntu\nID_LIKE=debian\n`);
    assert.equal(c.apt_based, true);
    assert.equal(c.id, "ubuntu");
  });

  it("classifies debian as apt-based", () => {
    const c = classifyOsRelease(`ID=debian\n`);
    assert.equal(c.apt_based, true);
    assert.equal(c.id, "debian");
  });

  it("classifies fedora (ID_LIKE=rhel fedora) as non-apt", () => {
    const c = classifyOsRelease(`ID=fedora\nID_LIKE="rhel fedora"\n`);
    assert.equal(c.apt_based, false);
    assert.equal(c.id, "fedora");
    assert.match(c.id_like, /rhel/);
  });

  it("classifies opensuse as non-apt", () => {
    const c = classifyOsRelease(`ID="opensuse-leap"\nID_LIKE="suse opensuse"\n`);
    assert.equal(c.apt_based, false);
    assert.equal(c.id, "opensuse-leap");
  });

  it("classifies arch as non-apt", () => {
    const c = classifyOsRelease(`ID=arch\nID_LIKE=archlinux\n`);
    assert.equal(c.apt_based, false);
    assert.equal(c.id, "arch");
  });

  it("classifies alpine as non-apt", () => {
    const c = classifyOsRelease(`ID=alpine\n`);
    assert.equal(c.apt_based, false);
    assert.equal(c.id, "alpine");
  });
});

describe("install-linux-build-deps", () => {
  it("passes --self-test (retry, ::error::, package contract)", () => {
    const result = spawnSync("bash", [script, "--self-test"], {
      encoding: "utf8",
      env: process.env,
    });
    assert.equal(
      result.status,
      0,
      `self-test failed:\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
    );
    assert.match(result.stdout, /install-linux-build-deps self-test: ok/);

    const onGha = process.env.GITHUB_ACTIONS === "true";
    const aptFamily = hostIsAptFamily();
    if (onGha && aptFamily) {
      assert.match(
        result.stdout,
        /ok: -o opts gate passes under later-sorted Retries 1 override/,
        "Debian/Ubuntu CI must run the apt-config -o opts gate (not skip)",
      );
    } else if (!aptFamily) {
      assert.match(
        result.stdout,
        /skip: apt gate not applicable on \S+ \(package manager: (apt|dnf|yum|zypper|pacman|apk|unknown)\)/,
        "non-apt hosts must emit the explicit skip: … not applicable line",
      );
    }
  });
});
