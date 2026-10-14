import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  autoPortInfoPorts,
  isEnableGraphUiDisabled,
  resolveGraphUiStatusMessage,
  shouldBlockEnableOnConflict,
} from "./graph-ui-button-state.ts";
import { GraphUiButtonView } from "./graph-ui-button-view.ts";

const labels = {
  enable: "Enable Graph UI",
  enableWithPort: "Enable Graph UI · 18750",
  open: "Open 3D Graph",
  indexFirst: "Index workspace first",
  busyEllipsis: "…",
};

describe("graph-ui-button-state", () => {
  it("enables the button in Auto mode even when a conflict flag is set", () => {
    assert.equal(
      isEnableGraphUiDisabled(false, { port_conflict: true, port_mode: "auto" }),
      false,
    );
    assert.equal(
      shouldBlockEnableOnConflict({ port_conflict: true, port_mode: "auto" }),
      false,
    );
  });

  it("disables the button in User mode with a conflict", () => {
    assert.equal(
      isEnableGraphUiDisabled(false, { port_conflict: true, port_mode: "user" }),
      true,
    );
    assert.equal(
      shouldBlockEnableOnConflict({ port_conflict: true, port_mode: "user" }),
      true,
    );
  });

  it("disables while busy regardless of mode", () => {
    assert.equal(
      isEnableGraphUiDisabled(true, { port_conflict: false, port_mode: "auto" }),
      true,
    );
  });

  it("exposes remap ports for the Auto info note", () => {
    assert.deepEqual(
      autoPortInfoPorts({
        port_mode: "auto",
        port: 18750,
        remap_from_port: 18749,
      }),
      { busy: 18749, next: 18750 },
    );
    assert.equal(
      autoPortInfoPorts({
        port_mode: "user",
        port: 18749,
        remap_from_port: 18749,
      }),
      null,
    );
  });

  it("resolveGraphUiStatusMessage translates key+params, remap, and fallback", () => {
    const translate = (key: string, params?: Record<string, string | number>) =>
      `${key}:${JSON.stringify(params ?? {})}`;

    assert.equal(
      resolveGraphUiStatusMessage(
        {
          port: 18749,
          message_key: "graphMsgForeignUiUser",
          message_params: { port: 18749 },
          conflict_message: null,
          info_message: null,
          remap_from_port: null,
        },
        translate,
      ),
      'graphMsgForeignUiUser:{"port":18749,"next":18749}',
    );

    assert.equal(
      resolveGraphUiStatusMessage(
        {
          port: 18750,
          message_key: "graphAutoPortInfo",
          message_params: null,
          conflict_message: null,
          info_message: null,
          remap_from_port: 18749,
        },
        translate,
      ),
      'graphAutoPortInfo:{"busy":18749,"next":18750,"port":18750}',
    );

    assert.equal(
      resolveGraphUiStatusMessage(
        {
          port: 18749,
          message_key: null,
          message_params: null,
          conflict_message: "legacy conflict",
          info_message: null,
          remap_from_port: null,
        },
        translate,
      ),
      "legacy conflict",
    );

    assert.equal(
      resolveGraphUiStatusMessage(
        {
          port: 18749,
          message_key: null,
          message_params: null,
          conflict_message: null,
          info_message: "  info only  ",
          remap_from_port: null,
        },
        translate,
      ),
      "info only",
    );
  });
});

describe("GraphUiButtonView", () => {
  it("keeps Enable clickable in Auto with info note and renders the note", () => {
    const html = renderToStaticMarkup(
      createElement(GraphUiButtonView, {
        status: {
          port: 18750,
          port_conflict: false,
          conflict_message: null,
          info_message: "18749 busy",
          remap_from_port: 18749,
          port_mode: "auto",
          ui_available: false,
          project_indexed: false,
        },
        busy: false,
        infoNote: "18749 is in use by another process; 18750 will be used",
        conflictNote: null,
        labels,
        onEnable: () => undefined,
        onOpen: () => undefined,
      }),
    );
    assert.match(html, /data-qa="graph-ui-enable"/);
    assert.doesNotMatch(html, /disabled=""/);
    assert.match(html, /data-qa="graph-ui-auto-info"/);
    assert.match(html, /18750 will be used/);
    assert.match(html, /Enable Graph UI · 18750/);
  });

  it("disables Enable in User mode and renders the translated conflict reason", () => {
    const reason =
      "Port 18749 is serving another codebase-memory-mcp (for example Antigravity). Lounge will not adopt it. In User mode the port is not changed automatically — pick another port in Settings.";
    const html = renderToStaticMarkup(
      createElement(GraphUiButtonView, {
        status: {
          port: 18749,
          port_conflict: true,
          conflict_message: null,
          message_key: "graphMsgForeignUiUser",
          message_params: { port: 18749 },
          info_message: null,
          remap_from_port: null,
          port_mode: "user",
          ui_available: false,
          project_indexed: false,
        },
        busy: false,
        infoNote: null,
        conflictNote: reason,
        labels,
        onEnable: () => undefined,
        onOpen: () => undefined,
      }),
    );
    assert.match(html, /data-qa="graph-ui-enable"/);
    assert.match(html, /disabled=""/);
    assert.match(html, /data-qa="graph-ui-conflict-note"/);
    assert.match(html, /Antigravity/);
    assert.match(html, /title="[^"]*Antigravity[^"]*"/);
    assert.doesNotMatch(html, /data-qa="graph-ui-auto-info"/);
  });

  it("renders conflict note for Auto band-exhausted (port_conflict)", () => {
    const reason = "Graph UI port band 18749–18749 is full. Free a port or choose a fixed port.";
    const html = renderToStaticMarkup(
      createElement(GraphUiButtonView, {
        status: {
          port: 18749,
          port_conflict: true,
          conflict_message: null,
          message_key: "graphMsgBandExhausted",
          message_params: { start: 18749, end: 18749, port: 18749 },
          info_message: null,
          remap_from_port: null,
          port_mode: "auto",
          ui_available: false,
          project_indexed: false,
        },
        busy: false,
        infoNote: null,
        conflictNote: reason,
        labels,
        onEnable: () => undefined,
        onOpen: () => undefined,
      }),
    );
    assert.match(html, /data-qa="graph-ui-conflict-note"/);
    assert.match(html, /band 18749/);
    assert.doesNotMatch(html, /disabled=""/);
  });
});
