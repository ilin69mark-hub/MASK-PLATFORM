import assert from "node:assert/strict";
import test from "node:test";

import {
  bridgeToProfile,
  repairProfile,
  widenToProfile,
  type Bridge,
  type RepairOutcome,
  type RepairProfile,
  type Widen,
} from "../lib/editor/repair.ts";
import type { EffectiveLimits, RepairResult, Shapes } from "../lib/kernel.ts";

/**
 * A complete `EffectiveLimits`, so a field added to the Rust struct breaks this
 * file rather than quietly going unchecked.
 */
const LIMITS: EffectiveLimits = {
  line_width_mm: 0.8,
  gap_mm: 1.0,
  island_width_mm: 3.0,
  island_area_mm2: 25.0,
  feature_length_mm: 1.5,
  bridge_length_mm: 25.0,
  corner_radius_mm: 0.5,
  curve_tolerance_mm: 0.02,
  min_angle_deg: 30.0,
  bridge_width_mm: 1.2,
  max_features_per_100cm2: 400.0,
  max_area_mm2: 1_000_000.0,
  max_tile_width_mm: 600.0,
  max_tile_length_mm: 1200.0,
  kerf_mm: 0.1,
  registration_error_mm: 0.3,
  registration_budget_mm: 0.25,
  peel_angle_min_deg: 30.0,
  peel_rate: "any",
};

const PROFILE: RepairProfile = {
  lineWidthMm: 0.8,
  bridgeWidthMm: 1.2,
  bridgeLengthMm: 25.0,
};

const SHAPES: Shapes = {
  shapes: [{ outer: { points: [[0, 0], [10, 0], [10, 5]], closed: true }, holes: [] }],
};

/** Identity by serialisation — the same fallback `history.ts` uses. */
const identity = (s: Shapes): string => JSON.stringify(s);

const widened: Shapes = {
  shapes: [{ outer: { points: [[0, 0], [10, 0], [10, 5.6]], closed: true }, holes: [] }],
};

test("the repair reads its numbers from the engine's limits, not from its own defaults", () => {
  const profile = repairProfile(LIMITS);
  assert.equal(profile.lineWidthMm, 0.8);
  assert.equal(profile.bridgeWidthMm, 1.2);
  assert.equal(profile.bridgeLengthMm, 25.0);
});

test("widening targets the width the validator compares against, not the island minimum", () => {
  // `island_width_mm` is 3.0 and `line_width_mm` is 0.8. Widening to the island
  // minimum would inflate every layer by three times; widening to anything below
  // `line_width_mm` is the failure BUG-019 describes.
  assert.ok(LIMITS.island_width_mm > LIMITS.line_width_mm);
  const seen: number[] = [];
  const widen: Widen = (s, target) => {
    seen.push(target);
    return { shapes: widened };
  };
  widenToProfile(SHAPES, repairProfile(LIMITS), widen, identity);
  assert.deepEqual(seen, [0.8]);
});

test("a widening that changed the geometry reports the change and the new shape", () => {
  const outcome = widenToProfile(SHAPES, PROFILE, () => ({ shapes: widened }), identity);
  assert.equal(outcome.kind, "widened");
  assert.deepEqual(outcome.shapes, widened);
  assert.match(outcome.message, /0\.800/);
});

test("a layer that needs no widening reports nothing rather than a change", () => {
  // The kernel returns the same hash when every region is already at or above
  // the target. Reporting `widened` here would push an undo step that undoes
  // nothing visible — the exact symptom `history.ts` dedups against.
  const outcome = widenToProfile(SHAPES, PROFILE, () => ({ shapes: SHAPES }), identity);
  assert.equal(outcome.kind, "nothing");
  assert.deepEqual(outcome.shapes, SHAPES);
  assert.match(outcome.message, /нечего/);
});

test("a refusal from the kernel reaches the caller rather than half-fixing the layer", () => {
  const widen: Widen = () => {
    throw new Error("widen: region 0 could not be grown: offset collapsed the contour");
  };
  assert.throws(
    () => widenToProfile(SHAPES, PROFILE, widen, identity),
    /could not be grown/
  );
});

test("bridging passes the profile's own tab width and length", () => {
  const seen: [number, number][] = [];
  const bridge: Bridge = (_s, width, maxLength) => {
    seen.push([width, maxLength]);
    return { shapes: SHAPES, bridges: [{ width_mm: width, length_mm: 4, at: { x: 0, y: 0 } }] };
  };
  bridgeToProfile(SHAPES, PROFILE, bridge);
  // These two are what T-06 and T-07 check. A tab narrower than 1.2 or longer
  // than 25 is one the validator reports on the spot.
  assert.deepEqual(seen, [[1.2, 25.0]]);
});

test("bridging reports each tab it placed, not a bare count", () => {
  const bridge: Bridge = () => ({
    shapes: widened,
    bridges: [
      { width_mm: 1.2, length_mm: 4.0, at: { x: 1, y: 1 } },
      { width_mm: 1.4, length_mm: 9.5, at: { x: 8, y: 2 } },
    ],
  });
  const outcome = bridgeToProfile(SHAPES, PROFILE, bridge);
  assert.equal(outcome.kind, "bridged");
  if (outcome.kind !== "bridged") return;
  assert.equal(outcome.bridges.length, 2);
  assert.deepEqual(outcome.bridges[1], { width_mm: 1.4, length_mm: 9.5, at: { x: 8, y: 2 } });
  // The message quotes the worst tab against the profile's own limits, so the
  // user sees the number the validator will use.
  assert.match(outcome.message, /1\.400/);
  assert.match(outcome.message, /9\.500/);
  assert.match(outcome.message, /1\.200/);
  assert.match(outcome.message, /25\.000/);
});

test("a layer with no free island reports nothing to anchor", () => {
  const bridge: Bridge = () => ({ shapes: SHAPES, bridges: [] });
  const outcome = bridgeToProfile(SHAPES, PROFILE, bridge);
  assert.equal(outcome.kind, "nothing");
  assert.deepEqual(outcome.shapes, SHAPES);
  assert.match(outcome.message, /островков/);
});

test("a tab list whose lengths are zero does not report a negative maximum", () => {
  // `Math.max` over an empty list is `-Infinity`; this guards the shape of the
  // guard above — no tabs must mean no tabs, not a tab-less message claiming a
  // maximum of negative infinity millimetres.
  const outcome = bridgeToProfile(SHAPES, PROFILE, () => ({ shapes: SHAPES, bridges: [] }));
  assert.doesNotMatch(outcome.message, /Infinity|NaN/);
});

test("the outcome is a closed union: no shape carries a `bridges` it cannot have", () => {
  // Compile-time property, asserted at runtime as documentation: a `nothing`
  // outcome from the widening path must not be readable as a bridge result.
  const widened_ = widenToProfile(SHAPES, PROFILE, () => ({ shapes: SHAPES }), identity);
  assert.equal("bridges" in widened_, false);
  const bridge_: RepairOutcome = bridgeToProfile(
    SHAPES,
    PROFILE,
    () => ({ shapes: SHAPES, bridges: [] }) satisfies RepairResult
  );
  assert.equal("bridges" in bridge_, false);
});