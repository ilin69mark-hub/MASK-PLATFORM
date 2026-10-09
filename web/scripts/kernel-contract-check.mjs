// Contract check between the TypeScript facade and the Rust kernel.
//
// A type mismatch here does not fail the build — TypeScript happily believes a
// wasm export returns whatever the .d.ts claims. It shows up as `undefined` at
// runtime, in the browser, in a feature nobody is testing. So the facade's
// assumptions are checked against the real artifact.
//
// Usage:
//   wasm-pack build --target nodejs --out-dir ../engine/wasm/pkg-node
//   node scripts/kernel-contract-check.mjs

import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { readFileSync } from "node:fs";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);

let failures = 0;
function check(name, condition, detail = "") {
  if (condition) {
    console.log(`ok   ${name}`);
  } else {
    failures += 1;
    console.log(`FAIL ${name}${detail ? `: ${detail}` : ""}`);
  }
}

/** The facade's shape for a measurement, transcribed from web/lib/kernel.ts. */
// An absent Rust `Option` crosses the boundary as `undefined`, so a nullable
// field is listed as "…|undefined" rather than "…|null". Getting this wrong is
// exactly the bug this script exists to catch: TypeScript believes the facade
// either way.
const FACADE_LAYER_RESULT = {
  area_mm2: "number",
  min_width_mm: "number|undefined",
  min_gap: "object|undefined",
  self_intersections: "array",
  island_count: "number",
  hole_count: "number",
  bbox: "array",
  content_hash: "string",
  path_data: "array",
};

const FACADE_CONSTANTS = [
  "canon_quantum_mm",
  "coord_dp",
  "measure_report_dp",
  "min_width_eps_mm",
  "max_coordinate_mm",
  "lib_scale",
  "lib_math_mode",
  "engine_version",
];

/**
 * The facade's shape for `effective_limits`, transcribed from web/lib/kernel.ts.
 *
 * Every field of `rules::EffectiveLimits`, with the type each one arrives as.
 * Eight were missing here while the kernel returned them all; nothing failed,
 * because TypeScript only objects to a field it wants and cannot find. The list
 * was an array of names checked for `typeof === "number"`, which is true of all
 * but one — `peel_rate` is free text by design, so the names have to carry their
 * expected type.
 */
const FACADE_LIMITS = {
  line_width_mm: "number",
  gap_mm: "number",
  island_width_mm: "number",
  island_area_mm2: "number",
  feature_length_mm: "number",
  bridge_length_mm: "number",
  corner_radius_mm: "number",
  curve_tolerance_mm: "number",
  min_angle_deg: "number",
  bridge_width_mm: "number",
  max_features_per_100cm2: "number",
  max_area_mm2: "number",
  max_tile_width_mm: "number",
  max_tile_length_mm: "number",
  kerf_mm: "number",
  registration_error_mm: "number",
  registration_budget_mm: "number",
  peel_angle_min_deg: "number",
  peel_rate: "string",
};

const FACADE_VALIDATION = [
  "verdict",
  "error_count",
  "warning_count",
  "info_count",
  "messages",
  "limits_physically_verified",
  "nested_layers",
];

const EXPORTS = [
  "engine_version",
  "engine_constants",
  "effective_limits",
  "measure_layer",
  "widen_centreline",
  "content_hash",
  "validate_project",
  "nest_and_validate_project",
  "move_vertex",
  "widen_shape_set",
  "bridge_unanchored_islands",
];

function typeOf(value) {
  if (value === null) return "null";
  if (Array.isArray(value)) return "array";
  return typeof value;
}

/**
 * Whether a value has one of the types a `…|undefined` expectation lists.
 *
 * `undefined` is always allowed: it is how an absent Rust `Option` arrives, and
 * a field that is legitimately optional must be allowed to be missing.
 */
function matchesType(value, expected) {
  return expected.split("|").includes(typeOf(value));
}

/** A shape's stored vertices, in the order the kernel serialised them. */
function storedPointsOf(shape) {
  return shape.outer.points.map((p) => [p.x, p.y]);
}

/** The vertices a canonical path draws, recovered from its numbers. */
function pointsInPath(d) {
  return d
    .match(/-?\d+(?:\.\d+)?/g)
    .map(Number)
    .reduce((pts, n, i) => {
      if (i % 2 === 0) pts.push([n]);
      else pts[pts.length - 1].push(n);
      return pts;
    }, []);
}

async function main() {
  const wasm = require(
    resolve(here, "..", "..", "engine", "wasm", "pkg-node", "mask_wasm.js")
  );

  const material = readFileSync(
    resolve(here, "..", "..", "engine", "profiles", "data", "matte-vinyl-090.v4.json"),
    "utf8"
  );
  const plotter = readFileSync(
    resolve(here, "..", "..", "engine", "profiles", "data", "knife-25mm.v2.json"),
    "utf8"
  );

  // ── every function the facade calls must exist ───────────────────────
  for (const name of EXPORTS) {
    check(`export present: ${name}`, typeof wasm[name] === "function");
  }

  // ── the grid constant duplicated in the facade ───────────────────────
  const constants = wasm.engine_constants();
  check(
    "facade MIN_WIDTH_EPS_MM matches the kernel",
    constants.min_width_eps_mm === 1e-3,
    `kernel says ${constants.min_width_eps_mm}`
  );
  check(
    "facade canonical grid matches the kernel",
    constants.canon_quantum_mm === 1e-4,
    `kernel says ${constants.canon_quantum_mm}`
  );

  // ── constants shape ──────────────────────────────────────────────────
  for (const key of FACADE_CONSTANTS) {
    check(
      `constants.${key} present`,
      key in constants && constants[key] !== undefined,
      typeof constants[key]
    );
  }

  // ── limits shape ─────────────────────────────────────────────────────
  const limits = wasm.effective_limits(material, plotter);
  for (const [key, expected] of Object.entries(FACADE_LIMITS)) {
    check(
      `effective_limits.${key} present`,
      key in limits && matchesType(limits[key], expected),
      typeof limits[key]
    );
  }

  // ── measurement shape ────────────────────────────────────────────────
  const square = JSON.stringify({
    shapes: [
      {
        outer: { points: [[0, 0], [10, 0], [10, 5], [0, 5]], closed: true },
        holes: [],
      },
    ],
  });
  const measured = wasm.measure_layer(square);
  // A single square: one region, so the nullable fields must actually be absent.
  // Measuring a two-region layer instead would let a facade that always got a
  // value pass a check about what happens when there is none.
  for (const [key, expected] of Object.entries(FACADE_LAYER_RESULT)) {
    const allowed = expected.split("|");
    check(
      `measure_layer.${key} matches ${expected}`,
      allowed.includes(typeOf(measured[key])),
      `got ${typeOf(measured[key])} (${measured[key]})`
    );
  }
  check(
    "measure_layer.bbox has four numbers",
    Array.isArray(measured.bbox) && measured.bbox.length === 4 &&
      measured.bbox.every((v) => typeof v === "number"),
    JSON.stringify(measured.bbox)
  );
  check(
    "measure_layer.path_data holds strings",
    Array.isArray(measured.path_data) && measured.path_data.every((d) => typeof d === "string")
  );

  // ── validation shape ─────────────────────────────────────────────────
  const project = JSON.stringify({
    composition: {
      layers: [
        {
          index: 1,
          name: "Base",
          geometry: {
            shapes: [
              {
                outer: { points: [[0, 0], [100, 0], [100, 100], [0, 100]], closed: true },
                holes: [],
              },
            ],
          },
        },
      ],
    },
  });
  const verdict = wasm.validate_project(project, material, plotter);
  for (const key of FACADE_VALIDATION) {
    check(
      `validate_project.${key} present`,
      key in verdict && verdict[key] !== undefined,
      typeof verdict[key]
    );
  }
  check(
    "verdict is one of the four documented values",
    ["PASS", "PASS_WITH_WARNINGS", "FAIL", "FATAL"].includes(verdict.verdict),
    verdict.verdict
  );
  check(
    "messages is an array of strings",
    Array.isArray(verdict.messages) && verdict.messages.every((m) => typeof m === "string")
  );

  // ── the laid-out call must differ from the un-nested one in a stated way ──
  //
  // T-15 can only see geometry when the project is laid out first. If the two
  // calls returned the same answer for a project that needs nesting, the facade
  // would be describing a check that is not running — and TypeScript would not
  // notice, because `nested_layers` is just a field.
  const wide = JSON.stringify({
    composition: {
      layers: [
        {
          index: 1,
          name: "Base",
          geometry: {
            shapes: [
              {
                outer: {
                  points: [[0, 0], [400, 0], [400, 400], [0, 400]],
                  closed: true,
                },
                holes: [],
              },
            ],
          },
        },
      ],
    },
  });
  const unNested = wasm.validate_project(wide, material, plotter);
  const nested = wasm.nest_and_validate_project(wide, material, plotter);
  check(
    "an un-nested layer that does not fit is reported",
    unNested.messages.some((m) => m.includes("\u0440\u0430\u0431\u043e\u0447\u0435\u0435 \u043f\u043e\u043b\u0435")),
    JSON.stringify(unNested.messages)
  );
  check(
    "the laid-out call reports which layers it laid out",
    Array.isArray(nested.nested_layers) && nested.nested_layers.includes(1),
    JSON.stringify(nested.nested_layers)
  );
  check(
    "the laid-out call no longer claims the layer does not fit",
    !nested.messages.some((m) => m.includes("\u0440\u0430\u0431\u043e\u0447\u0435\u0435 \u043f\u043e\u043b\u0435")),
    JSON.stringify(nested.messages)
  );
  check(
    "and it says so in words, not only in the field list",
    nested.messages.some((m) => m.includes("\u0440\u0430\u0437\u0431\u0438\u0442")),
    JSON.stringify(nested.messages)
  );

  // ── the write path must go through the kernel ────────────────────────
  //
  // A vertex move is the first thing the editor does to geometry. If the facade
  // could do it in JavaScript, the drawn shape and the measured shape would be
  // two different objects, and the disagreement would only show up at submission.
  // So the check is not "the export exists" — it is that the kernel's own numbers
  // come back from a move.
  const before = wasm.measure_layer(square).content_hash;
  const moved = wasm.move_vertex(square, {
    shape: 0,
    contour: 0,
    vertex: 1,
    x_mm: 12.0,
    y_mm: 0.0,
  });
  check(
    "move_vertex returns canonical geometry and its measurement",
    moved.shapes &&
      Array.isArray(moved.shapes.shapes) &&
      typeof moved.measured.area_mm2 === "number" &&
      typeof moved.measured.content_hash === "string" &&
      Array.isArray(moved.measured.path_data),
    Object.keys(moved).join(",")
  );
  const storedPoints = storedPointsOf(moved.shapes.shapes[0]);
  const drawnPoints = pointsInPath(moved.measured.path_data[0]);
  check(
    "the geometry it returns is the geometry it measured",
    JSON.stringify(storedPoints) === JSON.stringify(drawnPoints),
    `stored ${JSON.stringify(storedPoints)}, drawn ${JSON.stringify(drawnPoints)}`
  );

  check(
    "the moved path starts from the moved vertex",
    moved.measured.path_data[0].startsWith("M 0.0000 0.0000 L 12.0000 0.0000"),
    moved.measured.path_data[0]
  );
  check(
    "the bounding box follows the move",
    moved.measured.bbox[2] === 12,
    JSON.stringify(moved.measured.bbox)
  );
  check(
    "the content hash changes when a vertex moves",
    typeof before === "string" && moved.measured.content_hash !== before,
    `before ${before}, after ${moved.measured.content_hash}`
  );

  let refused = "";
  try {
    wasm.move_vertex(square, { shape: 0, contour: 0, vertex: 1, x_mm: 0, y_mm: 0 });
  } catch (e) {
    refused = String(e && e.message ? e.message : e);
  }
  check(
    "a move onto a neighbour is refused, not silently merged",
    refused.includes("would land on vertex"),
    refused
  );

  // ── the repairs must run in this runtime, not just exist ─────────────
  //
  // Same reasoning as the write path. A repair that only existed as an export
  // would leave the editor's buttons wired to something the browser runtime
  // cannot do — and the user would find out by pressing them.
  const whisker = JSON.stringify({
    shapes: [
      {
        outer: { points: [[0, 0], [100, 0], [100, 0.2], [0, 0.2]], closed: true },
        holes: [],
      },
    ],
  });
  const widened = wasm.widen_shape_set(whisker, limits.line_width_mm);
  check(
    "widen_shape_set returns geometry measured against the target",
    widened.shapes &&
      typeof widened.measured.min_width_mm === "number" &&
      widened.measured.min_width_mm >= limits.line_width_mm - 1e-3,
    `min_width_mm ${widened.measured.min_width_mm}, target ${limits.line_width_mm}`
  );
  const widenedStored = storedPointsOf(widened.shapes.shapes[0]);
  const widenedDrawn = pointsInPath(widened.measured.path_data[0]);
  check(
    "the geometry it returns is the geometry it measured",
    JSON.stringify(widenedStored) === JSON.stringify(widenedDrawn),
    `stored ${JSON.stringify(widenedStored)}, drawn ${JSON.stringify(widenedDrawn)}`
  );

  const alreadyWide = wasm.widen_shape_set(square, limits.line_width_mm);
  check(
    "a region already wide enough comes back untouched",
    alreadyWide.measured.content_hash === wasm.measure_layer(square).content_hash,
    `before ${wasm.measure_layer(square).content_hash}, after ${alreadyWide.measured.content_hash}`
  );

  const frame = JSON.stringify({
    shapes: [
      {
        outer: { points: [[0, 0], [200, 0], [200, 200], [0, 200]], closed: true },
        holes: [{ points: [[3, 3], [3, 197], [197, 197], [197, 3]], closed: true }],
      },
      {
        outer: { points: [[6, 6], [6, 10], [10, 10], [10, 6]], closed: true },
        holes: [],
      },
    ],
  });
  const bridged = wasm.bridge_unanchored_islands(
    frame,
    limits.bridge_width_mm,
    limits.bridge_length_mm
  );
  check(
    "bridge_unanchored_islands returns the tabs it placed",
    Array.isArray(bridged.bridges) && bridged.bridges.length > 0,
    JSON.stringify(bridged.bridges)
  );
  const tabsPass = bridged.bridges.every(
    (b) =>
      typeof b.width_mm === "number" &&
      typeof b.length_mm === "number" &&
      typeof b.at?.x === "number" &&
      typeof b.at?.y === "number"
  );
  check("every tab carries the two numbers T-06 and T-07 read", tabsPass, JSON.stringify(bridged.bridges));
  check(
    "a tab's position is an {x, y} object, not a [x, y] tuple",
    bridged.bridges.every((b) => typeof b.at?.x === "number" && typeof b.at?.y === "number"),
    // The facade once declared `at: [number, number]`. It type-checked against a
    // value that never arrives, and `at[0]` on it would have been undefined in
    // the browser while `at.x` worked in a Rust test.
    JSON.stringify(bridged.bridges.map((b) => Object.keys(b.at ?? {})))
  );
  check(
    "no tab is narrower or longer than the profile allows",
    bridged.bridges.every(
      (b) =>
        b.width_mm >= limits.bridge_width_mm - 1e-9 &&
        b.length_mm <= limits.bridge_length_mm + 1e-9
    ),
    `limits ${limits.bridge_width_mm}/${limits.bridge_length_mm}, tabs ${JSON.stringify(
      bridged.bridges.map((b) => [b.width_mm, b.length_mm])
    )}`
  );
  check(
    "the tab joins the island to the frame: one region fewer",
    bridged.shapes.shapes.length === 1,
    `${bridged.shapes.shapes.length} regions`
  );

  let uncuttable = "";
  try {
    wasm.bridge_unanchored_islands(frame, 0, limits.bridge_length_mm);
  } catch (e) {
    uncuttable = String(e && e.message ? e.message : e);
  }
  check(
    "an uncuttable tab is refused rather than silently placing none",
    uncuttable.length > 0,
    uncuttable
  );

  // ── the whole round trip: repair, then validate what was repaired ─────
  //
  // VALIDATE-001 exists because `validate()` had no call site at all. The
  // question this answers is not "is the verdict right" — that is the
  // validator's own tests — but "does a layer the user just repaired come back
  // as a verdict, rather than as a document nobody looked at". An unanchored
  // island must be reported before bridging and quiet after it; if it were
  // quiet both times, the tabs are not reaching the rules that read them.
  const asBundle = (shapes, bridges) => ({
    composition: { layers: [{ index: 0, name: "рама", geometry: shapes, bridges }] },
  });
  const freeIsland = wasm.validate_project(
    JSON.stringify(asBundle(JSON.parse(frame), [])),
    material,
    plotter
  );
  // The claim under test is about *this* complaint, not about the message list
// being empty. Two notices are unconditional — the release is not modelled, and
// the project has no curves — and they are in both runs. An assertion of
// `messages.length === 0` would have been red for a reason that has nothing to
// do with tabs, which is the failure this script exists to catch.
const UNANCHORED = "не соединён с маской";
check(
    "an unanchored island is reported by the browser validator",
    freeIsland.messages.some((m) => m.includes(UNANCHORED)),
    JSON.stringify(freeIsland.messages)
  );

  const bridgedVerdict = wasm.validate_project(
    JSON.stringify(asBundle(bridged.shapes, bridged.bridges)),
    material,
    plotter
  );
  check(
    "bridging it clears the complaint the tabs answered",
    !bridgedVerdict.messages.some((m) => m.includes(UNANCHORED)),
    JSON.stringify(bridgedVerdict.messages)
  );
  check(
    "the notices that have nothing to do with tabs stay put",
    bridgedVerdict.messages.length < freeIsland.messages.length &&
      bridgedVerdict.messages.some((m) => m.includes("Кривых в проекте нет")),
    `${freeIsland.messages.length} before, ${bridgedVerdict.messages.length} after`
  );
  check(
    "and the verdict itself is one of the four the engine defines",
    ["PASS", "PASS_WITH_WARNINGS", "FAIL", "FATAL"].includes(bridgedVerdict.verdict),
    bridgedVerdict.verdict
  );

  const nestedVerdict = wasm.nest_and_validate_project(
    JSON.stringify(asBundle(bridged.shapes, bridged.bridges)),
    material,
    plotter
  );
  check(
    "the laid-out call answers with a verdict too, not only the plain one",
    typeof nestedVerdict.verdict === "string" && nestedVerdict.verdict.length > 0,
    JSON.stringify(nestedVerdict)
  );

  let noGeometry = "";
  try {
    wasm.validate_project(
      JSON.stringify({ composition: { layers: [{ index: 0, name: "без геометрии" }] } }),
      material,
      plotter
    );
  } catch (e) {
    noGeometry = String(e && e.message ? e.message : e);
  }
  check(
    "a layer without inline geometry is refused, per ADR-0016",
    noGeometry.includes("inline geometry"),
    noGeometry
  );

  console.log();
  if (failures > 0) {
    console.log(`RESULT: ${failures} contract failure(s)`);
    process.exit(1);
  }
  console.log("RESULT: the TypeScript facade matches the kernel artifact");
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});