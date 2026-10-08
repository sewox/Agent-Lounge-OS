import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { GraphUiButtonView } from "../components/graph-ui-button.tsx";
import {
  autoPortInfoPorts,
  isEnableGraphUiDisabled,
  shouldBlockEnableOnConflict,
} from "./graph-ui-button-state.ts";

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

  it("disables Enable in User mode with conflict", () => {
    const html = renderToStaticMarkup(
      createElement(GraphUiButtonView, {
        status: {
          port: 18749,
          port_conflict: true,
          conflict_message: "Port 18749 busy",
          info_message: null,
          remap_from_port: null,
          port_mode: "user",
          ui_available: false,
          project_indexed: false,
        },
        busy: false,
        infoNote: null,
        labels,
        onEnable: () => undefined,
        onOpen: () => undefined,
      }),
    );
    assert.match(html, /data-qa="graph-ui-enable"/);
    assert.match(html, /disabled=""/);
    assert.doesNotMatch(html, /data-qa="graph-ui-auto-info"/);
  });
});
