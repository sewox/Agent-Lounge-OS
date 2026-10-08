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

function hasAptConfig(): boolean {
  const result = spawnSync("bash", ["-c", "command -v apt-config"], {
    encoding: "utf8",
    env: process.env,
  });
  return result.status === 0;
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

  it("classifies CRLF ubuntu os-release as apt-based", () => {
    const c = classifyOsRelease(`ID=ubuntu\r\nID_LIKE=debian\r\n`);
    assert.equal(c.apt_based, true);
    assert.equal(c.id, "ubuntu");
  });
});

describe("install-linux-build-deps non-apt refusal", () => {
  for (const distro of [
    { name: "fedora", body: `ID=fedora\nID_LIKE="rhel fedora"\n` },
    { name: "alpine", body: `ID=alpine\n` },
  ]) {
    it(`refuses ${distro.name} with ::error:: and zero sudo calls`, () => {
      const dir = fs.mkdtempSync(path.join(os.tmpdir(), "apt-refuse-"));
      const osRelease = path.join(dir, "os-release");
      fs.writeFileSync(osRelease, distro.body, "utf8");
      const sudoLog = path.join(dir, "sudo.log");
      const bin = path.join(dir, "bin");
      fs.mkdirSync(bin);
      const sudoStub = path.join(bin, "sudo");
      fs.writeFileSync(
        sudoStub,
        `#!/usr/bin/env bash\nprintf '%s\\n' "sudo $*" >>${JSON.stringify(sudoLog)}\nexit 0\n`,
        "utf8",
      );
      fs.chmodSync(sudoStub, 0o755);

      const result = spawnSync("bash", [script], {
        encoding: "utf8",
        env: {
          ...process.env,
          PATH: `${bin}${path.delimiter}${process.env.PATH ?? ""}`,
          CI_APT_OS_RELEASE_PATH: osRelease,
        },
      });
      assert.notEqual(result.status, 0, "non-apt host must exit non-zero");
      const combined = `${result.stdout}\n${result.stderr}`;
      assert.match(
        combined,
        /::error::install-linux-build-deps\.sh is for Debian\/Ubuntu apt only/,
      );
      assert.match(combined, new RegExp(`Detected ID=${distro.name}`));
      assert.equal(
        fs.existsSync(sudoLog),
        false,
        "sudo must not be invoked on non-apt refusal",
      );
      fs.rmSync(dir, { recursive: true, force: true });
    });
  }
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
    assert.match(
      result.stdout,
      /ok: mirror backup\/switch surface cp\/sed failures with ::error::/,
    );

    const onGha = process.env.GITHUB_ACTIONS === "true";
    const aptFamily = hostIsAptFamily();
    const aptConfig = hasAptConfig();

    if (aptFamily && aptConfig) {
      // Debian/Ubuntu CI and local Debian/Ubuntu with apt-config: gate must run.
      assert.match(
        result.stdout,
        /ok: -o opts gate passes under later-sorted Retries 1 override/,
        "Debian/Ubuntu with apt-config must run the -o opts gate",
      );
    } else if (aptFamily && !aptConfig && !onGha) {
      assert.match(
        result.stdout,
        /skip: apt-config missing on \S+ \(non-CI\); -o opts gate not run \(package manager: /,
        "local Debian/Ubuntu without apt-config must say apt-config missing (not 'not applicable')",
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
