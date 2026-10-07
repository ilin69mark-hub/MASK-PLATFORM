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

import { StatusBar } from "../components/StudioCanvas.tsx";
import type { LayerResult } from "../lib/kernel.ts";

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
