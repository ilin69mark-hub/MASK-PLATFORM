// Tests for the TypeScript facade against the real WASM artifact.
//
// This runs the same module the browser runs (`web/lib/kernel.ts`), with the
// artifact injected instead of fetched. Node strips the types, so no build step
// stands between the test and the code under test.
//
// Usage:
//   wasm-pack build --target nodejs --out-dir ../engine/wasm/pkg-node
//   node --experimental-strip-types tests/kernel.test.ts

import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import {
  __setKernelForTests,
  constants,
  hash,
  isLoaded,
  loadKernel,
  measure,
  pathDataOf,
  rect,
  rectCW,
  reportMm,
  shapeOf,
  toShapes,
  union,
  version,
  widen,
} from "../lib/kernel.ts";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);

const PKG = resolve(here, "..", "..", "engine", "wasm", "pkg-node", "mask_wasm.js");
const MATERIAL = resolve(
  here,
  "..",
  "..",
  "engine",
  "profiles",
  "data",
  "matte-vinyl-090.v4.json"
);
const PLOTTER = resolve(
  here,
  "..",
  "..",
  "engine",
  "profiles",
  "data",
  "knife-25mm.v2.json"
);

/** Loads the built artifact and hands it to the facade. */
async function boot() {
  if (isLoaded()) return;
  await loadKernel(async () => require(PKG));
}

const square = toShapes(shapeOf(rect(0, 0, 10, 5)));

test("the kernel loads and reports a version", async () => {
  await boot();
  assert.match(version(), /^\d+\.\d+\.\d+$/);
});

test("constants arrive as a plain object, not a Map", async () => {
  await boot();
  const c = constants();
  // The regression this guards: serde_wasm_bindgen returns a Map by default, so
  // property access yields undefined while get() works. That split is invisible
  // until a screen renders an empty number.
  assert.equal(typeof c.canon_quantum_mm, "number");
  assert.equal(c.canon_quantum_mm, 1e-4);
  assert.equal(c.lib_math_mode, "Integer");
});

test("measuring a square returns its area and narrowest element", async () => {
  await boot();
  const m = measure(square);
  assert.ok(Math.abs(m.area_mm2 - 50) < 1e-9, `area ${m.area_mm2}`);
  assert.ok(Math.abs((m.min_width_mm ?? 0) - 5) < 1e-3, `width ${m.min_width_mm}`);
  assert.equal(m.island_count, 1);
  assert.equal(m.hole_count, 0);
});

test("the gap between two regions reaches the editor", async () => {
  // T-02. The facade had no way to see this at all, so a user dragging one
  // element toward another got no feedback until submission.
  await boot();
  const two = toShapes(
    shapeOf(rect(0, 0, 5, 5)),
    shapeOf(rect(8, 0, 5, 5))
  );
  const r = measure(two);
  assert.ok(r.min_gap, "two regions must report a gap");
  assert.ok(Math.abs(r.min_gap.mm - 3) < 1e-6, `got ${r.min_gap.mm}`);
  assert.deepEqual([r.min_gap.a, r.min_gap.b], [0, 1]);
});

test("a single region reports no gap rather than a zero", async () => {
  // A gap against nothing would be a number invented, not measured. The absence
  // arrives as `undefined`, not `null`, so the facade must be checked against what
  // actually crosses the boundary.
  await boot();
  const r = measure(square);
  assert.equal(r.min_gap, undefined);
  assert.notEqual(r.min_gap, null);
});

test("a contour crossing itself is named, not measured", async () => {
  // The failure the CLI's integrity gate exists to stop. Signed area cancels two
  // equal lobes and reports 0 mm², which reads as a measurement rather than an
  // error, and the editor used to show exactly that.
  await boot();
  const bowTie = toShapes({
    outer: { points: [{ x: 0, y: 0 }, { x: 10, y: 10 }, { x: 10, y: 0 }, { x: 0, y: 10 }], closed: true },
    holes: [],
  });
  const r = measure(bowTie);
  assert.ok(r.self_intersections.length > 0, "the crossing must be reported");
  const [x, y] = r.self_intersections[0]!.at;
  assert.ok(Math.abs(x - 5) < 1e-9 && Math.abs(y - 5) < 1e-9, `got ${x},${y}`);
});

test("ordinary geometry reports no crossings", async () => {
  await boot();
  assert.deepEqual(measure(square).self_intersections, []);
});

test("path data comes back canonical", async () => {
  await boot();
  const m = measure(square);
  assert.equal(
    m.path_data[0],
    "M 0.0000 0.0000 L 10.0000 0.0000 L 10.0000 5.0000 L 0.0000 5.0000 Z"
  );
});

test("a hole is counted separately from the region", async () => {
  await boot();
  const withHole = toShapes(
    // The hole must wind clockwise or the kernel rejects it, which is the point.
    shapeOf(rect(0, 0, 20, 20), [rectCW(5, 5, 10, 10)])
  );
  const m = measure(withHole);
  assert.equal(m.hole_count, 1);
  // Signed area: 400 outer minus 100 hole.
  assert.ok(Math.abs(m.area_mm2 - 300) < 1e-6, `area ${m.area_mm2}`);
});

test("a hole wound the same way as its outer contour is rejected", async () => {
  await boot();
  // Signed area would add the hole and report 500 mm² for a true 300 mm². A wrong
  // area is worse than no answer: it reaches fidelity, material estimates and the
  // manifest, all of which record it as fact.
  assert.throws(
    () => measure(toShapes(shapeOf(rect(0, 0, 20, 20), [rect(5, 5, 10, 10)]))),
    /clockwise/
  );
});

test("out-of-bounds geometry raises a catchable error", async () => {
  await boot();
  assert.throws(() =>
    measure(toShapes(shapeOf(rect(0, 0, 1e12, 10))))
  );
});

test("widening produces the requested band", async () => {
  await boot();
  const band = widen(
    [
      [0, 0],
      [100, 0],
    ],
    0.8
  );
  assert.ok(Math.abs((band.min_width_mm ?? 0) - 0.8) < 1e-3);
});

test("touching bands merge into one region", async () => {
  await boot();
  const a = toShapes(shapeOf(rect(0, 0, 10, 10)));
  const b = toShapes(shapeOf(rect(10, 0, 10, 10)));
  const merged = union(a, b);
  const m = measure(merged);
  assert.equal(m.island_count, 1, "touching shapes are one piece of film");
  assert.ok(Math.abs(m.area_mm2 - 200) < 1e-6);
});

test("hashing is deterministic and distinguishes geometry", async () => {
  await boot();
  const h1 = hash(square);
  const h2 = hash(toShapes(shapeOf(rect(0, 0, 10, 5))));
  assert.equal(h1, h2);
  assert.match(h1, /^sha256:[0-9a-f]{64}$/);
  assert.notEqual(h1, hash(toShapes(shapeOf(rect(0, 0, 10, 6)))));
});

test("sub-quantum jitter does not change the hash", async () => {
  await boot();
  // Below the canonical grid, so it must not alter identity.
  const a = hash(toShapes(shapeOf(rect(0, 0, 10, 5))));
  const b = hash(toShapes(shapeOf(rect(0, 0, 10 + 1e-9, 5))));
  assert.equal(a, b);
});

test("reporting a length matches the kernel's precision", async () => {
  await boot();
  assert.equal(reportMm(0.42), "0.420 mm");
  assert.equal(reportMm(0.42, 2), "0.42 mm");
});

test("effective limits come back with every documented key", async () => {
  await boot();
  const { limits } = await import("../lib/kernel.ts");
  const { readFileSync } = await import("node:fs");
  const l = limits(
    readFileSync(MATERIAL, "utf8"),
    readFileSync(PLOTTER, "utf8")
  );
  for (const key of [
    "line_width_mm",
    "gap_mm",
    "island_width_mm",
    "corner_radius_mm",
    "bridge_width_mm",
    "max_features_per_100cm2",
    "max_area_mm2",
    "max_tile_width_mm",
    "max_tile_length_mm",
    "kerf_mm",
    "registration_error_mm",
  ]) {
    assert.equal(typeof l[key as keyof typeof l], "number", `${key} missing`);
  }
  assert.equal(l.line_width_mm, 0.8);
  assert.equal(l.max_tile_width_mm, 284);
});

test("the local path helper agrees with the kernel's canonical form", async () => {
  await boot();
  // pathDataOf is only for geometry the kernel has not seen. When both paths run
  // over the same shape they must agree, or the preview would differ from the
  // export.
  const m = measure(square);
  assert.deepEqual(pathDataOf(square), m.path_data);
});

test("an empty contour produces no path", () => {
  assert.deepEqual(pathDataOf(toShapes({ outer: { points: [], closed: true }, holes: [] })), []);
});

test("calling before load gives a clear error", () => {
  // Reset, then call without loading.
  __setKernelForTests(null);
  assert.throws(() => version(), /not loaded/);
});