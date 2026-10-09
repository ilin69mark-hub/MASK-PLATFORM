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
import { NodeTool } from "../components/NodeTool.tsx";
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
        bridges: [{ width_mm: 1.2, length_mm: 4.5, at: { x: 0, y: 0 } }],
        message: "Язычков: 1 · ширина до 1.200 мм (минимум 1.200) · длина до 4.500 мм (максимум 25.000).",
      },
    })
  );

  assert.ok(html.includes("Язычков: 1"));
  assert.ok(html.includes("1.200"));
  assert.ok(html.includes("4.500"));
  assert.ok(html.includes("25.000"));
});

test("the verdict is shown as an answer, with the thresholds' own caveat", () => {
  // A PASS over unverified limits is the editor speaking for the material. The
  // datasheet line is what stops a green headline meaning "cut this".
  const html = renderToStaticMarkup(
    React.createElement(StudioCanvas, {
      ...CANVAS,
      verdict: {
        kind: "verdict",
        headline: "Годно к вырезанию",
        messages: [],
        nestedLayers: [],
        limitsVerified: false,
      },
    })
  );

  assert.ok(html.includes('data-testid="verdict-panel"'));
  assert.ok(html.includes("Годно к вырезанию"));
  assert.ok(html.includes('data-testid="limits-unverified"'));
  assert.ok(html.includes("datasheet"));
  assert.ok(!html.includes('role="alert"'), "a verdict is an answer, not an emergency");
});

test("a verified profile drops the caveat rather than repeating it as fact", () => {
  const html = renderToStaticMarkup(
    React.createElement(StudioCanvas, {
      ...CANVAS,
      verdict: {
        kind: "verdict",
        headline: "Годно к вырезанию",
        messages: [],
        nestedLayers: [],
        limitsVerified: true,
      },
    })
  );
  assert.ok(!html.includes('data-testid="limits-unverified"'));
});

test("the layers a layout had to move are named, not just counted", () => {
  // A verdict that improved because a layer was cut into tiles reads as "it
  // always passed" unless the reader can see which layers moved.
  const html = renderToStaticMarkup(
    React.createElement(StudioCanvas, {
      ...CANVAS,
      verdict: {
        kind: "verdict",
        headline: "Годно с замечаниями",
        messages: ["слой 2 разложен"],
        nestedLayers: [2, 5],
        limitsVerified: true,
      },
    })
  );
  assert.ok(html.includes("разложено слоёв: 2, 5"));
  assert.ok(html.includes("слой 2 разложен"));
});

test("a refusal reads as a refusal and shows its reason", () => {
  const html = renderToStaticMarkup(
    React.createElement(StudioCanvas, {
      ...CANVAS,
      verdict: {
        kind: "refused",
        headline: "Проверка не выполнена",
        messages: ["Геометрия слоя «панель» изменилась после постановки язычков."],
        nestedLayers: [],
        limitsVerified: false,
      },
    })
  );
  assert.ok(html.includes("Проверка не выполнена"));
  assert.ok(html.includes("изменилась после постановки язычков"));
});

test("no verdict yet is not the same as a clean one", () => {
  // The editor used to show measurements and nothing else, so a mask on screen
  // looked like a mask that had been checked.
  const html = renderToStaticMarkup(React.createElement(StudioCanvas, CANVAS));
  assert.ok(!html.includes('data-testid="verdict-panel"'));
});

test("the grab dots appear only while the Node tool is on", () => {
  // They used to render unconditionally. A user pressed «Вершины», saw the dots
  // already there, took that as proof the tool was active — and it then refused
  // every drag. An affordance visible when it does nothing misreports the state.
  const off = renderToStaticMarkup(React.createElement(StudioCanvas, CANVAS));
  assert.ok(!off.includes('data-testid="node-tool"'), "no dots while the tool is off");

  // The tool is on when its toggle reads pressed; the owner cannot set it, so the
  // toggle's own state is what a reader has to go on.
  const on = renderToStaticMarkup(
    React.createElement(StudioCanvas, {
      ...CANVAS,
      onEdit: () => {},
      nodeToolOn: true,
    })
  );
  assert.ok(on.includes('data-testid="node-tool"'), "dots while the tool is on");
});

test("the Node toggle carries the state it is switched to", () => {
  // `aria-pressed` was already there and there was no rule styling it, so the
  // button looked identical in both states. This asserts the attribute the style
  // hangs off, so removing it breaks a test rather than the interface.
  const html = renderToStaticMarkup(
    React.createElement(StudioCanvas, {
      ...CANVAS,
      nodeToolOn: true,
      onEdit: () => {},
    })
  );
  assert.ok(
    /data-testid="node-tool-toggle"[^>]*aria-pressed="true"/.test(html) ||
      /aria-pressed="true"[^>]*data-testid="node-tool-toggle"/.test(html),
    `the toggle must announce it is on: ${html}`
  );
});

test("the grabbed vertex is red and larger than the rest", () => {
  // The only feedback a grab gives is this dot. It was already red in the source,
  // which is why its absence proved the grab was never happening rather than
  // that the highlight was missing. It is now also bigger, because a colour
  // change alone is easy to miss among two dozen green dots, and a dot that grows
  // under the cursor says which one is held without reading anything.
  const geometry: Shapes = {
    shapes: [
      {
        outer: {
          points: [
            { x: 0, y: 0 },
            { x: 10, y: 0 },
            { x: 10, y: 5 },
            { x: 0, y: 5 },
          ],
          closed: true,
        },
        holes: [],
      },
    ],
  };
  const grabbed = { shape: 0, contour: 0, vertex: 1, at: { x: 10, y: 0 } };
  const html = renderToStaticMarkup(
    React.createElement(NodeTool, {
      shapes: geometry,
      radiusMm: 1,
      grabbed,
      pointerMm: null,
    })
  );
  const dots = html.match(/<circle[^>]*>/g) ?? [];
  assert.equal(dots.length, 4);
  const held = dots.filter((d) => d.includes('data-grabbed="true"'));
  assert.equal(held.length, 1, `exactly one dot is held: ${html}`);
  assert.ok(held[0]?.includes('fill="#c41820"'), `held dot must be red: ${held[0]}`);
  assert.ok(held[0]?.includes('r="1.6"'), `held dot must be larger: ${held[0]}`);
  for (const d of dots) {
    if (!d.includes('data-grabbed')) assert.ok(d.includes('fill="#0a7"'), `held ones are red: ${d}`);
  }
});

test("with nothing grabbed no dot claims to be held", () => {
  const geometry: Shapes = {
    shapes: [{ outer: { points: [{ x: 0, y: 0 }, { x: 1, y: 1 }], closed: true }, holes: [] }],
  };
  const html = renderToStaticMarkup(
    React.createElement(NodeTool, {
      shapes: geometry,
      radiusMm: 1,
      grabbed: null,
      pointerMm: null,
    })
  );
  assert.ok(!html.includes('data-grabbed'), "nothing is held, so nothing says it is");
  assert.ok(!html.includes("#c41820"), "and nothing is red");
});
