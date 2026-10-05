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
const FACADE_LAYER_RESULT = {
  area_mm2: "number",
  min_width_mm: "number|null",
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
];

const EXPORTS = [
  "engine_version",
  "engine_constants",
  "effective_limits",
  "measure_layer",
  "widen_centreline",
  "content_hash",
  "validate_project",
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
  for (const [key, expected] of Object.entries(FACADE_LAYER_RESULT)) {
    check(
      `measure_layer.${key} matches ${expected}`,
      typeOf(measured[key]) === expected.replace("|", "") ||
        (expected.includes("|") &&
          (measured[key] === null || typeOf(measured[key]) === "number")),
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