/**
 * What the two auto-fixes are allowed to do, and what they are told afterwards.
 *
 * Pure apart from the injected kernel calls, for the reason `applyEdit.ts` gives:
 * WASM is not available in `node:test`, and the decisions worth testing — *which*
 * numbers go to the kernel, and whether a repair that changed nothing should
 * leave an undo step behind — are all here.
 *
 * Every number is taken from the profile the engine published, never from a
 * default in this file. That is the whole point of ADR-0013: a repair that used
 * its own idea of a tab width would place tabs the station would then report on
 * (T-06), and the editor would be certifying a mask that gets rejected.
 */

import type { EffectiveLimits, RepairResult, Shapes } from "@/lib/kernel";

/**
 * The three numbers a repair needs.
 *
 * Narrowed to what is used rather than taking `EffectiveLimits` whole, so a field
 * added to the kernel later cannot quietly become part of a repair's contract.
 */
export interface RepairProfile {
  /** Minimum element width — what the validator compares against (T-03). */
  lineWidthMm: number;
  /** Minimum tab width (T-06). */
  bridgeWidthMm: number;
  /** Longest cuttable tab (T-07). */
  bridgeLengthMm: number;
}

/**
 * The repair's numbers, read from the engine's own limits.
 *
 * `line_width_mm` is the width to widen *to*, deliberately and not
 * `island_width_mm`: BUG-019 established that the validator takes the minimum of
 * `min_width` and `narrowest_neck`, and the repair has to target the same number
 * or it would report success and leave the layer unmade.
 */
export function repairProfile(limits: EffectiveLimits): RepairProfile {
  return {
    lineWidthMm: limits.line_width_mm,
    bridgeWidthMm: limits.bridge_width_mm,
    bridgeLengthMm: limits.bridge_length_mm,
  };
}

/** One tab the repair placed, as the kernel measured it. */
export type BridgeRecord = RepairResult["bridges"][number];

/**
 * Identity for a shape set, for deciding whether a repair changed anything.
 *
 * The same rule `history.ts` falls back to when a value carries no kernel
 * `content_hash`, named here rather than repeated at each call site. Geometry
 * that came out of the kernel serialises in a deterministic field order, so this
 * comparison is stable; it is not the kernel's canonical hash, and nothing that
 * claims a content hash should be built on it.
 */
export function shapesIdentity(value: Shapes): string {
  return JSON.stringify(value);
}

/**
 * What a repair did, and what to tell the user.
 *
 * `nothing` is a real outcome, not an error: it means the geometry is already
 * what the repair would have produced. It is a separate kind because the caller
 * has to treat it differently — see {@link widenToProfile}.
 */
export type RepairOutcome =
  | {
      kind: "widened";
      shapes: Shapes;
      message: string;
    }
  | {
      kind: "bridged";
      shapes: Shapes;
      /** The tabs, in the form the validator checks. */
      bridges: readonly BridgeRecord[];
      message: string;
    }
  | {
      kind: "nothing";
      shapes: Shapes;
      message: string;
    };

/** The kernel call that widens a shape set. */
export type Widen = (shapes: Shapes, targetWidthMm: number) => { shapes: Shapes };

/** The kernel call that anchors free islands. */
export type Bridge = (
  shapes: Shapes,
  widthMm: number,
  maxLengthMm: number,
) => RepairResult;

/**
 * Widen every narrow region to the material's minimum width.
 *
 * The kernel refuses a target that is not a positive finite width, and leaves a
 * region already at or above the target alone; a layer that needs no widening
 * therefore comes back as the same geometry. That case is reported as `nothing`
 * rather than `widened`, because the caller pushes an undo step for a repair —
 * and an undo step for geometry that did not move is one that appears to do
 * nothing when pressed.
 *
 * The kernel throws when it cannot grow a region, and the throw reaches the
 * caller: a repair that half-succeeded would leave the editor showing geometry
 * the validator has not seen.
 */
export function widenToProfile(
  shapes: Shapes,
  profile: RepairProfile,
  widen: Widen,
  identity: (value: Shapes) => string = shapesIdentity,
): RepairOutcome {
  const widened = widen(shapes, profile.lineWidthMm);
  if (identity(widened.shapes) === identity(shapes)) {
    return {
      kind: "nothing",
      shapes,
      message: `Все элементы не уже ${profile.lineWidthMm.toFixed(3)} мм — расширять нечего.`,
    };
  }
  return {
    kind: "widened",
    shapes: widened.shapes,
    message: `Расширено до ${profile.lineWidthMm.toFixed(3)} мм — минимума материала.`,
  };
}

/**
 * Anchor every free island with a tab.
 *
 * Reports the tabs it placed rather than a bare count, because T-06 and T-07 are
 * checked against each tab's own width and length: showing the user the number
 * they will be judged on is the difference between a repair and a rumour.
 *
 * A layer with nothing to anchor comes back with no tabs, and is reported as
 * `nothing` for the reason given above. Note that this reads the tab list rather
 * than the geometry: a bridge that merged nothing cannot have placed a tab, so
 * the two agree by construction.
 */
export function bridgeToProfile(
  shapes: Shapes,
  profile: RepairProfile,
  bridge: Bridge,
): RepairOutcome {
  const repaired = bridge(shapes, profile.bridgeWidthMm, profile.bridgeLengthMm);

  if (repaired.bridges.length === 0) {
    return {
      kind: "nothing",
      shapes,
      message: "Свободных островков нет — язычки ставить некуда.",
    };
  }

  const widest = Math.max(...repaired.bridges.map((b) => b.width_mm));
  const longest = Math.max(...repaired.bridges.map((b) => b.length_mm));
  return {
    kind: "bridged",
    shapes: repaired.shapes,
    bridges: repaired.bridges,
    message:
      `Язычков: ${repaired.bridges.length} · ширина до ${widest.toFixed(3)} мм ` +
      `(минимум ${profile.bridgeWidthMm.toFixed(3)}) · длина до ` +
      `${longest.toFixed(3)} мм (максимум ${profile.bridgeLengthMm.toFixed(3)}).`,
  };
}