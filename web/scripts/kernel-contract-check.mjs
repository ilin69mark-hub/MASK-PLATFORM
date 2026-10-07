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

const FACADE_LIMITS = [
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
];

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
];

function typeOf(value) {
  if (value === null) return "null";
  if (Array.isArray(value)) return "array";
  return typeof value;
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
  for (const key of FACADE_LIMITS) {
    check(
      `effective_limits.${key} present`,
      key in limits && typeof limits[key] === "number",
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