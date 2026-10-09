// Render tests for the status bar itself.
//
// `web/lib/status.ts` already tests the decision: an error must win over a
// missing kernel. These tests prove the other half — that `StatusBar` turns that
// decision into markup a user can actually see, instead of leaving "Загрузка…"
// on screen for ever.
// Server-side rendering is enough: the branch under test is presentational, and
// `react-dom/server` needs no browser or DOM. The TSX loader in this directory
// transpiles the imported component with the TypeScript compiler already used by
// `web-typecheck`, so no test framework or browser dependency was added.
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { test } from "node:test";

import { StatusBar, StudioCanvas, type StudioProps } from "../components/StudioCanvas.tsx";
import type { LayerResult, Shapes } from "../lib/kernel.ts";

test("a kernel-load failure is rendered as an alert rather than loading", () => {
  const failure = "Mask kernel loaded but exposes no measure_layer()";
  const html = renderToStaticMarkup(
    React.createElement(StatusBar, {
      measurement: { status: "error", error: failure },
      kernelReady: false,
      minWidthMm: 0.8,
      minGapMm: 0.9,
      onDownload: () => {},
    })
  );

  assert.match(html, /role="alert"/);
  assert.match(html, /class="error"/);
  assert.ok(html.includes(failure), `rendered markup was: ${html}`);
  assert.ok(
    !html.includes("Загрузка геометрического ядра"),
    `a recorded failure must not leave loading text on screen: ${html}`
  );
});

test("ordinary loading still renders the waiting text without an alert", () => {
  const html = renderToStaticMarkup(
    React.createElement(StatusBar, {
      measurement: { status: "idle" },
      kernelReady: false,
      minWidthMm: 0.8,
      minGapMm: 0.9,
      onDownload: () => {},
    })
  );

  assert.ok(html.includes("Загрузка геометрического ядра"));
  assert.ok(!html.includes('role="alert"'));
});

test("a ready measurement renders its measured thresholds, not placeholders", () => {
  const result: LayerResult = {
    area_mm2: 12.5,
    min_width_mm: 0.4,
    min_gap: { a: 0, b: 1, mm: 0.5 },
    self_intersections: [],
    island_count: 2,
    hole_count: 1,
    bbox: [0, 0, 10, 5],
    content_hash: "sha256:0123456789abcdef0123456789abcdef",
    path_data: [],
  };
  const html = renderToStaticMarkup(
    React.createElement(StatusBar, {
      measurement: { status: "ready", result, elapsedMs: 12 },
      kernelReady: true,
      minWidthMm: 0.8,
      minGapMm: 0.9,
      onDownload: () => {},
    })
  );

  assert.ok(html.includes('data-testid="min-width"'));
  assert.ok(html.includes('data-testid="min-gap"'));
  assert.ok(html.includes("0.400 мм"));
  assert.ok(html.includes("0.500 мм"));
  assert.ok(html.includes("ниже минимума 0.800 мм"));
  assert.ok(html.includes("ниже минимума 0.900 мм"));
  assert.ok(html.includes("площадь"));
});

/**
 * The repair buttons, and the promise they make.
 *
 * A repair cut against a number the bench never used is worse than no repair at
 * all: the editor would place steel the station rejects. So the buttons exist but
 * stay disabled until the kernel has derived `effective_limits`, and the reason
 * a repair did nothing is on screen rather than inferred from the canvas.
 */
const EMPTY: Shapes = { shapes: [] };

const CANVAS: StudioProps = {
  shapes: EMPTY,
  projectId: "demo",
  revision: 1,
  minWidthMm: 0.8,
  minGapMm: 0.9,
};

test("the repair buttons are disabled until the profile's limits are known", () => {
  const html = renderToStaticMarkup(
    React.createElement(StudioCanvas, { ...CANVAS, canRepair: false, onWiden: () => {}, onBridge: () => {} })
  );

  assert.ok(html.includes('data-testid="widen-button"'), `rendered markup was: ${html}`);
  assert.ok(html.includes('data-testid="bridge-button"'));
  const disabled = html.match(/<button[^>]*data-testid="(?:widen|bridge)-button"[^>]*>/g) ?? [];
  assert.equal(disabled.length, 2);
  for (const button of disabled) {
    assert.ok(/\sdisabled=""/.test(button), `must not be usable yet: ${button}`);
  }
});

test("the repair buttons become usable once the limits are derived", () => {
  const html = renderToStaticMarkup(
    React.createElement(StudioCanvas, { ...CANVAS, canRepair: true, onWiden: () => {}, onBridge: () => {} })
  );

  const buttons = html.match(/<button[^>]*data-testid="(?:widen|bridge)-button"[^>]*>/g) ?? [];
  assert.equal(buttons.length, 2);
  for (const button of buttons) {
    assert.ok(!/\sdisabled=""/.test(button), `must be usable now: ${button}`);
  }
});

test("what a repair did is shown, including when it changed nothing", () => {
  // The `nothing` outcome is the one a user would otherwise read as a broken
  // button, so it is the one that most needs saying out loud.
  const html = renderToStaticMarkup(
    React.createElement(StudioCanvas, {
      ...CANVAS,
      canRepair: true,
      onWiden: () => {},
      onBridge: () => {},
      repair: {
        kind: "nothing",
        shapes: CANVAS.shapes,
        message: "Свободных островков нет — язычки ставить некуда.",
      },
    })
  );

  assert.ok(html.includes('role="status"'));
  assert.ok(html.includes("Свободных островков нет"));
  assert.ok(html.includes('data-testid="repair-report"'));
});

test("a placed tab is reported with its own measurements, against the profile's limits", () => {
  const html = renderToStaticMarkup(
    React.createElement(StudioCanvas, {
      ...CANVAS,
      canRepair: true,
      onWiden: () => {},
      onBridge: () => {},
      repair: {
        kind: "bridged",
        shapes: CANVAS.shapes,
        bridges: [{ width_mm: 1.2, length_mm: 4.5, at: [0, 0] }],
        message: "Язычков: 1 · ширина до 1.200 мм (минимум 1.200) · длина до 4.500 мм (максимум 25.000).",
      },
    })
  );

  assert.ok(html.includes("Язычков: 1"));
  assert.ok(html.includes("1.200"));
  assert.ok(html.includes("4.500"));
  assert.ok(html.includes("25.000"));
});
