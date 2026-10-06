// Tests for the status-bar decision, extracted from StudioCanvas.
//
// The ordering bug these cover was invisible: the component asked whether the kernel
// was ready before it asked whether loading had failed, and the load handler recorded
// the failure without marking itself ready. The user read "Загрузка…" for ever.
//
// `--experimental-strip-types` strips types but does not transform JSX, so a `.tsx`
// component cannot be imported by this runner at all. That is why the decision lives
// in a plain module: testing it here is possible without adding a DOM environment to
// the project. Rendering the markup is not covered — that is TEST-004.

import assert from "node:assert/strict";
import { test } from "node:test";

import {
  belowMinimum,
  shownMm,
  statusView,
  type MeasurementState,
} from "../lib/status.ts";

const ready = <T,>(result: T): MeasurementState<T> => ({ status: "ready", result });
const failed = (error: string): MeasurementState<never> => ({ status: "error", error });

test("a recorded failure is shown even when the kernel never became ready", () => {
  // The exact defect: `setKernelReady(true)` was only called on success, so a failed
  // load left the component not-ready with an error recorded — and the not-ready
  // guard came first.
  const view = statusView(false, failed("Mask kernel loaded but exposes no measure_layer()"));
  assert.equal(view.kind, "error");
  assert.match(view.text, /measure_layer/);
});

test("loading is shown before the kernel has answered, with nothing wrong", () => {
  const view = statusView(false, { status: "idle" });
  assert.equal(view.kind, "loading");
});

test("a kernel still loading wins over an in-progress measurement", () => {
  const view = statusView(false, { status: "measuring" });
  assert.equal(view.kind, "loading");
});

test("a ready kernel with a pending measurement says so", () => {
  const view = statusView(true, { status: "measuring" });
  assert.equal(view.kind, "measuring");
});

test("a failure is shown even after the kernel has loaded", () => {
  // Measurement and load are separate failures; this one must not be mistaken for a
  // stale measurement either.
  const view = statusView(true, failed("coordinate 1e400 is not finite"));
  assert.equal(view.kind, "error");
  assert.match(view.text, /not finite/);
});

test("a ready measurement carries its result rather than only announcing one", () => {
  // Returning the fact without the thing left the component narrowing the state a
  // second time by hand, and that second narrowing is where the two could disagree.
  const view = statusView(true, ready({ area_mm2: 50 }));
  assert.equal(view.kind, "metrics");
  assert.deepEqual((view as { result: { area_mm2: number } }).result, { area_mm2: 50 });
});

test("the comparison band keeps a layer on the limit from flickering", () => {
  const eps = 1e-3;
  // ADR-0004: an element 0.7995 mm wide against a 0.80 mm minimum passes.
  assert.equal(belowMinimum(0.7995, 0.8, eps), false);
  assert.equal(belowMinimum(0.7989, 0.8, eps), true);
  assert.equal(belowMinimum(0.8, 0.8, eps), false);
});

test("an unmeasured value is not a failure", () => {
  // A single region has no narrowest element and no neighbour to be close to. Zero
  // would fail every minimum, which would be reporting an absence as a measurement.
  assert.equal(belowMinimum(undefined, 0.8, 1e-3), false);
});

test("an unmeasured value is shown as a dash and not as zero", () => {
  assert.equal(shownMm(undefined), "—");
  assert.equal(shownMm(0.4), "0.400 мм");
  assert.equal(shownMm(0.4, 2), "0.40 мм");
});
