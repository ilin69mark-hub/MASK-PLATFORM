// @ts-check
/**
 * What the status bar shows, decided without rendering anything.
 *
 * This is here because the answer was once wrong in a way no test could see. The
 * component asked "is the kernel ready?" before it asked "did loading fail?", and
 * the load handler recorded the failure without marking itself ready — so a user
 * with no wasm artifact read "Загрузка геометрического ядра…" for ever, with the
 * message sitting unreachable behind the guard.
 *
 * Two things went wrong that a reading of the component would not have caught: the
 * ordering of two guards, and the state not carrying the meaning. Moving the
 * decision here puts it in one place, in a plain TypeScript file that the test
 * runner can import — `node --experimental-strip-types` strips types but does not
 * transform JSX, so a `.tsx` component cannot be tested by the runner at all, and
 * the alternative was a DOM test environment that the project does not have.
 *
 * This is not component testing and does not claim to be: it covers the decision,
 * not the markup. Rendering the markup is `TEST-004`.
 */

/**
 * What a measurement attempt is doing.
 *
 * Generic over the result type so the kernel's own `LayerResult` flows through
 * without a cast, and so the module stays independent of the facade.
 */
export type MeasurementState<R = unknown> =
  | { status: "idle" }
  | { status: "measuring" }
  | { status: "ready"; result: R; elapsedMs?: number }
  | { status: "error"; error: string };

/**
 * The four things the status bar can say.
 *
 * `metrics` carries the result rather than merely announcing one. Returning the fact
 * without the thing left the component narrowing the state a second time by hand, and
 * that second narrowing is where a mismatch between the two would have lived.
 */
export type StatusView<R = unknown> =
  | { kind: "loading"; text: string }
  | { kind: "error"; text: string }
  | { kind: "measuring"; text: string }
  | { kind: "metrics"; result: R; elapsedMs?: number | undefined };

const LOADING = "Загрузка геометрического ядра…";
const MEASURING = "Измерение…";

/**
 * Decide what the status bar shows.
 *
 * The error is asked about first, and that order is the whole point. Loading is
 * the default state and failure is the exception to it, so a state machine that
 * checks the exception first cannot strand a recorded failure behind the default —
 * whatever the caller forgot to set. A caller that records an error and nothing else
 * still gets the error on screen.
 *
 * @param kernelReady whether the kernel has finished loading, successfully or not
 * @param measurement the latest measurement state
 */
export function statusView<R>(
  kernelReady: boolean,
  measurement: MeasurementState<R>
): StatusView<R> {
  if (measurement.status === "error") {
    return { kind: "error", text: measurement.error };
  }
  if (!kernelReady) {
    return { kind: "loading", text: LOADING };
  }
  if (measurement.status === "ready") {
    return {
      kind: "metrics",
      result: measurement.result,
      elapsedMs: measurement.elapsedMs,
    };
  }
  return { kind: "measuring", text: MEASURING };
}

/**
 * Whether a measured width is below the profile minimum.
 *
 * The epsilon band is the kernel's own (`MIN_WIDTH_EPS_MM`, ADR-0004). An element
 * 0.7995 mm wide against a 0.80 mm minimum passes, and without the band a layer
 * sitting exactly on the limit flickers between pass and fail.
 *
 * `undefined` means there was nothing to measure — a single region has no
 * narrowest element and no neighbour to be close to — and is not a failure.
 *
 * @param measured the value from the kernel
 * @param threshold the profile minimum
 * @param eps the comparison band
 */
export function belowMinimum(
  measured: number | undefined,
  threshold: number,
  eps: number
): boolean {
  return measured !== undefined && measured < threshold - eps;
}

/**
 * How a measurement is shown, or `undefined` when there is nothing to show.
 *
 * An em dash rather than `0.000`: reporting zero for a value that was never
 * measured states a measurement that did not happen.
 */
export function shownMm(measured: number | undefined, dp = 3): string {
  return measured === undefined ? "—" : `${measured.toFixed(dp)} мм`;
}