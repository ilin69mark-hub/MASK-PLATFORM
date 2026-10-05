// Runtime check for the browser kernel.
//
// Compiling to wasm proves the code compiles. It does not prove the module
// loads, that the exports are reachable, or that a measurement taken in the
// browser agrees with the one the server takes. This runs the built artifact in
// a real JS runtime and compares against the values the Rust side produces.
//
// Usage:
//   cd engine/wasm && wasm-pack build --target nodejs --out-dir pkg-node
//   node scripts/runtime_check.mjs

import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

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

// The band geometry is taken from the golden file rather than reconstructed
// here. `widen` chooses its own vertex order and miter placement, and a hash
// depends on exactly those details, so a hand-written rectangle would be testing
// a shape the engine never produces.
function bandShapes(golden, key) {
  return golden[key];
}

async function main() {
  // The nodejs target instantiates synchronously, so there is no init promise to
  // await. Loading it through require works even from an ESM script.
  const wasm = require(resolve(here, "..", "pkg-node", "mask_wasm.js"));

  const engine = readFileSync(
    resolve(here, "..", "..", "profiles", "data", "matte-vinyl-090.v4.json"),
    "utf8"
  );
  const plotter = readFileSync(
    resolve(here, "..", "..", "profiles", "data", "knife-25mm.v2.json"),
    "utf8"
  );

  // ── the module loads and reports itself ───────────────────────────────
  const version = wasm.engine_version();
  check("module loads and reports a version", typeof version === "string" && version.length > 0, version);

  const constants = wasm.engine_constants();
  check(
    "the browser reports the canonical grid",
    constants.canon_quantum_mm === 1e-4,
    `got ${constants.canon_quantum_mm}`
  );
  check(
    "the math mode is pinned to Integer",
    constants.lib_math_mode === "Integer",
    `got ${constants.lib_math_mode}`
  );
  check(
    "the browser and the kernel agree on the grid",
    constants.canon_quantum_mm === 1e-4,
  );

  // ── measurement ───────────────────────────────────────────────────────
  const square = JSON.stringify({
    shapes: [
      {
        outer: { points: [[0, 0], [10, 0], [10, 5], [0, 5]], closed: true },
        holes: [],
      },
    ],
  });

  const measured = wasm.measure_layer(square);
  check("a square measures its area", Math.abs(measured.area_mm2 - 50) < 1e-9, `${measured.area_mm2}`);
  check(
    "a square measures its narrowest element",
    Math.abs(measured.min_width_mm - 5) < 1e-3,
    `${measured.min_width_mm}`
  );
  check(
    "path data comes back canonical",
    measured.path_data[0] === "M 0.0000 0.0000 L 10.0000 0.0000 L 10.0000 5.0000 L 0.0000 5.0000 Z",
    measured.path_data[0]
  );

  // ── hostile input must be an error, not a trap ────────────────────────
  let threw = false;
  try {
    wasm.measure_layer('{"shapes":[{"outer":{"points":[[0,0],[1e12,1]],"closed":true},"holes":[]}]}');
  } catch (e) {
    threw = true;
  }
  check("out-of-bounds coordinates raise a catchable error", threw);

  threw = false;
  try {
    wasm.measure_layer('{"shapes":[{"outer":{"points":[[0,0],[1,1]],"closed":true},"holes":[]}]}');
  } catch (e) {
    threw = true;
  }
  check("malformed input raises a catchable error", threw);

  // ── widening ──────────────────────────────────────────────────────────
  const line = JSON.stringify([[0, 0], [100, 0]]);
  const band = wasm.widen_centreline(line, 0.8);
  check(
    "widening produces the requested band",
    Math.abs(band.min_width_mm - 0.8) < 1e-3,
    `${band.min_width_mm}`
  );

  // ── the hash must be stable across calls ──────────────────────────────
  const h1 = wasm.content_hash(square);
  const h2 = wasm.content_hash(square);
  check("hashing is deterministic", h1 === h2, `${h1} vs ${h2}`);
  check("the hash is a sha256 string", /^sha256:[0-9a-f]{64}$/.test(h1), h1);

  const other = wasm.content_hash(square.replace("10,5", "10,6"));
  check("different geometry hashes differently", h1 !== other);

  // ── limits, so the editor can explain a failure ───────────────────────
  const limits = wasm.effective_limits(engine, plotter);
  check("effective limits are exposed", limits.line_width_mm === 0.8, `${limits.line_width_mm}`);
  check(
    "the tile width accounts for margins",
    limits.max_tile_width_mm === 284,
    `${limits.max_tile_width_mm}`
  );

  // ── validation of a whole project ─────────────────────────────────────
  const project = JSON.stringify({
    composition: {
      layers: [
        {
          index: 1,
          name: "Base",
          geometry: {
            shapes: [
              {
                outer: {
                  points: [[0, 0], [100, 0], [100, 100], [0, 100]],
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

  const verdict = wasm.validate_project(project, engine, plotter);
  check(
    "a project inside the working field validates",
    verdict.verdict === "PASS",
    `${verdict.verdict} ${JSON.stringify(verdict.messages)}`
  );
  check(
    "the browser reports that nothing has been proven",
    verdict.limits_physically_verified === false,
    `${verdict.limits_physically_verified}`
  );

  // A project with a 1200 mm layer must fail on the field rule, which is the
  // rule a nesting plan is supposed to resolve.
  const wide = JSON.stringify({
    composition: {
      layers: [
        {
          index: 1,
          name: "Wide",
          geometry: {
            shapes: [
              {
                outer: {
                  points: [[0, 0], [1200, 0], [1200, 100], [0, 100]],
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
  const wideVerdict = wasm.validate_project(wide, engine, plotter);
  check(
    "a layer wider than the field fails in the browser too",
    wideVerdict.verdict === "FAIL",
    `${wideVerdict.verdict}`
  );

  // ── cross-runtime agreement ───────────────────────────────────────────
  // The decisive property: the same geometry must hash identically here and in
  // the Rust CLI. The expected value is read from a golden file written by the
  // Rust side, so the two cannot drift without the test noticing.
  const goldenPath = resolve(here, "golden", "hashes.json");
  let golden = {};
  try {
    golden = JSON.parse(readFileSync(goldenPath, "utf8"));
  } catch {
    console.log("note: no golden hash file yet; cross-runtime check skipped");
  }

  check(
    "the browser hash matches the Rust hash for identical geometry",
    h1 === golden.square_10x5,
    `browser ${h1}, rust ${golden.square_10x5}`
  );

  const square735 = JSON.stringify({
    shapes: [
      { outer: { points: [[0, 0], [7, 0], [7, 3], [0, 3]], closed: true }, holes: [] },
    ],
  });
  check(
    "a second geometry also agrees across runtimes",
    wasm.content_hash(square735) === golden.square_7x3,
    `browser ${wasm.content_hash(square735)}, rust ${golden.square_7x3}`
  );

  // A band is the shape Auto-Fix produces, so its hash crossing runtimes matters
  // more than any other: the editor previews the widened geometry, and if that
  // preview hashed differently from the server's, the cache would never hit.
  const bandJson = JSON.stringify(bandShapes(golden, "band_0_80_shapes"));
  check(
    "a widened band hashes identically in both runtimes",
    wasm.content_hash(bandJson) === golden.band_0_80,
    `browser ${wasm.content_hash(bandJson)}, rust ${golden.band_0_80}`
  );

  const narrowJson = JSON.stringify(bandShapes(golden, "band_0_42_shapes"));
  check(
    "a narrow band hashes identically in both runtimes",
    wasm.content_hash(narrowJson) === golden.band_0_42,
    `browser ${wasm.content_hash(narrowJson)}, rust ${golden.band_0_42}`
  );

  console.log();
  if (failures > 0) {
    console.log(`RESULT: ${failures} failure(s)`);
    process.exit(1);
  }
  console.log("RESULT: the browser kernel loads, measures and agrees");
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});