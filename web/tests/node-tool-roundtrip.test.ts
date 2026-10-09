// The Node tool's draw-to-hit round trip, on the fixture the studio actually opens.
//
// Everything between "the vertex is drawn here" and "the pointer grabs that
// vertex" runs through three modules that are individually tested and jointly
// untested: the view is built from the measurement, the viewBox is fitted into an
// element with `preserveAspectRatio="meet"`, and the pointer is converted back
// through the same letterbox. A mismatch anywhere in that chain makes the dots
// land in one place and the hit test look in another, and the symptom is exactly
// the one this file exists to rule out: dots on screen, nothing drags.
//
// The chain is checked with the real WASM artifact, the real fixture and real
// numbers — millimetres to pixels to millimetres — rather than with hand-picked
// coordinates that would agree with anything.
//
// Usage:
//   wasm-pack build --target nodejs --out-dir ../engine/wasm/pkg-node
//   node --experimental-strip-types tests/node-tool-roundtrip.test.ts

import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import {
  __setKernelForTests,
  isLoaded,
  loadKernel,
  measure,
  union,
  widen,
  type Shapes,
} from "../lib/kernel.ts";
import { hitVertex, verticesOf } from "../lib/editor/hitTest.ts";
import {
  letterbox,
  mmToPx,
  pxRadiusToMm,
  pxToMm,
  type DrawnSizePx,
  type ViewBoxMm,
} from "../lib/editor/viewTransform.ts";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
const ROOT = resolve(here, "..", "..");

const PKG = resolve(ROOT, "engine/wasm/pkg-node/mask_wasm.js");
const FIXTURE = resolve(ROOT, "engine/fixtures/proof_panel.json");

async function boot() {
  if (isLoaded()) return;
  await loadKernel(async () => require(PKG));
}

interface FixtureLayer {
  index: number;
  name: string;
  width_mm: number;
  centrelines: [number, number][][];
}

/** The first layer of the fixture the studio opens. */
function firstFixtureLayer(): FixtureLayer {
  const fixture = JSON.parse(readFileSync(FIXTURE, "utf8")) as { layers: FixtureLayer[] };
  const layer = fixture.layers[0];
  assert.ok(layer, "the fixture has a first layer");
  return layer;
}

/** Build one fixture layer exactly the way `StudioFromFixture` does. */
function buildLayer(layer: FixtureLayer): Shapes {
  const parts: Shapes[] = [];
  for (const line of layer.centrelines) {
    const band = widen(line, layer.width_mm);
    for (const d of band.path_data) {
      const nums = (d.match(/-?\d+(?:\.\d+)?/g) ?? []).map(Number);
      const points = [];
      for (let i = 0; i + 1 < nums.length; i += 2) {
        points.push({ x: nums[i] ?? 0, y: nums[i + 1] ?? 0 });
      }
      parts.push({ shapes: [{ outer: { points, closed: true }, holes: [] }] });
    }
  }
  const first = parts[0];
  if (!first) return { shapes: [] };
  let merged = first;
  for (const part of parts.slice(1)) {
    merged = union(merged, part);
  }
  return merged;
}

/** The view `StudioCanvas` builds from a measurement. */
function viewOf(shapes: Shapes): ViewBoxMm {
  const [minX, minY, w, h] = measure(shapes).bbox;
  const pad = Math.max(w, h) * 0.04 || 5;
  return { minX: minX - pad, minY: minY - pad, w: w + pad * 2, h: h + pad * 2 };
}

/** A plausible element size: what the stylesheet gives the canvas at 100% width. */
const SIZES: DrawnSizePx[] = [
  { width: 1000, height: 650 },
  { width: 570, height: 380 },
  { width: 380, height: 650 },
  { width: 1600, height: 900 },
];

test("every drawn vertex is grabbable back at every element size", async () => {
  await boot();
  const layer = firstFixtureLayer();
  const shapes = buildLayer(layer);
  const vertices = verticesOf(shapes);
  assert.ok(vertices.length >= 8, `the fixture layer should have vertices, got ${vertices.length}`);

  const view = viewOf(shapes);
  const radiusPx = 6;

  for (const drawn of SIZES) {
    const box = letterbox(view, drawn);
    const grabRadiusMm = pxRadiusToMm(radiusPx, view, drawn);

    // Sanity on the fixture itself, so a failure below is about the chain and
    // not about a geometry too small to aim at.
    assert.ok(
      grabRadiusMm > 0.5,
      `a 6 px grab must be worth aiming at, got ${grabRadiusMm.toFixed(4)} mm at ${drawn.width}x${drawn.height}`
    );
    assert.ok(box.scale > 0, "the letterbox needs a scale");

    let missed = 0;
    let wrong = 0;
    for (const v of vertices) {
      // Exactly what the browser does: the dot is drawn at `v.at`, and the
      // pointer lands on the pixel that dot occupies.
      const px = mmToPx(v.at, view, drawn);
      const back = pxToMm(px, view, drawn);

      const hit = hitVertex(shapes, back, { radiusMm: grabRadiusMm });
      if (hit === null) {
        missed += 1;
        continue;
      }
      if (hit.shape !== v.shape || hit.contour !== v.contour || hit.vertex !== v.vertex) {
        wrong += 1;
      }
    }

    assert.equal(
      missed,
      0,
      `${missed} of ${vertices.length} drawn vertices were not grabbable at ${drawn.width}x${drawn.height} ` +
        `(grab radius ${grabRadiusMm.toFixed(3)} mm)`
    );
    assert.equal(
      wrong,
      0,
      `${wrong} of ${vertices.length} vertices grabbed the wrong one at ${drawn.width}x${drawn.height}`
    );
  }
});

test("a pointer a pixel away from a vertex still grabs it", async () => {
  await boot();
  const layer = firstFixtureLayer();
  const shapes = buildLayer(layer);
  const view = viewOf(shapes);
  const drawn: DrawnSizePx = { width: 1000, height: 650 };
  const grabRadiusMm = pxRadiusToMm(6, view, drawn);
  const scale = letterbox(view, drawn).scale;

  const v = verticesOf(shapes)[0];
  assert.ok(v, "the layer has a first vertex");

  // One pixel off, in millimetres — the user's own imprecision, and the reason
  // the radius exists at all.
  const onePixelMm = 1 * scale;
  const off = { x: v.at.x + onePixelMm, y: v.at.y + onePixelMm };

  const hit = hitVertex(shapes, off, { radiusMm: grabRadiusMm });
  assert.ok(hit, `a vertex ${(Math.hypot(onePixelMm, onePixelMm) * 1000).toFixed(0)} µm away must be found`);
  assert.deepEqual([hit.shape, hit.contour, hit.vertex], [v.shape, v.contour, v.vertex]);
});

test("two vertices exactly between each other are refused rather than guessed", async () => {
  // The band is 3 mm wide, so its two walls carry corners a few millimetres
  // apart. Aiming exactly between them is refused by design — and it must not
  // silently become "the nearest one", which would make the tool grab a
  // different vertex depending on which way the pointer drifted.
  await boot();
  const layer = firstFixtureLayer();
  const shapes = buildLayer(layer);
  const view = viewOf(shapes);
  const drawn: DrawnSizePx = { width: 1000, height: 650 };
  const grabRadiusMm = pxRadiusToMm(6, view, drawn);

  const vertices = verticesOf(shapes);
  let ties = 0;
  for (let i = 0; i < vertices.length && ties === 0; i += 1) {
    for (let j = i + 1; j < vertices.length; j += 1) {
      const a = vertices[i];
      const b = vertices[j];
      if (!a || !b) continue;
      const distance = Math.hypot(a.at.x - b.at.x, a.at.y - b.at.y);
      if (distance > grabRadiusMm) continue;
      const middle = { x: (a.at.x + b.at.x) / 2, y: (a.at.y + b.at.y) / 2 };
      if (hitVertex(shapes, middle, { radiusMm: grabRadiusMm }) === null) {
        ties += 1;
        break;
      }
    }
  }
  assert.ok(
    ties > 0,
    "the fixture has corners inside the grab radius, so a tie must be reachable"
  );
});

// Keeps the injected stub import from being flagged as unused if the facade ever
// stops needing it; the artifact is loaded through `loadKernel` above.
void __setKernelForTests;