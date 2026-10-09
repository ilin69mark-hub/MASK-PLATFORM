/**
 * Building the evaluation bundle the browser validator reads.
 *
 * This is **not** a Mask Project. A production Mask Project carries a coordinate
 * frame and `geometry_id` references and no vectors at all (ADR-0016); the browser
 * entry needs the vectors. So the thing built here is the separate evaluation
 * bundle ADR-0016 calls for, and the distinction is the whole reason it has a name
 * and a shape of its own. A consumer that believes it is validating a production
 * document is validating something else.
 *
 * Pure, so the rules below are testable in `node:test` without WASM. Two of them
 * exist because the alternative is a green verdict for a mask the station rejects,
 * which is the failure ADR-0013 was written about:
 *
 * - **Tabs must travel with the geometry.** Once a tab is merged, nothing in the
 *   contour distinguishes it from any other neck. T-06 and T-07 check the records
 *   the bridge pass produced, so a bundle that carries the geometry but drops the
 *   records validates a mask with no tabs at all and says nothing wrong with it.
 * - **Records that no longer describe the geometry must not travel.** Dragging a
 *   vertex after bridging changes the geometry and leaves the records claiming tabs
 *   at positions that are no longer there. Shipping them would have the validator
 *   check a claim, and dropping them silently would check nothing. Neither is
 *   honest, so the bundle is refused and the user is told what to do.
 */

import type { Shapes, TabRecord } from "@/lib/kernel";

/** One layer's contribution to the bundle. */
export interface BundleLayer {
  index: number;
  name: string;
  shapes: Shapes;
  /**
   * The tabs this geometry was built with, or `null` when none were placed.
   *
   * `null` and not `[]` so "never bridged" is distinguishable from "bridged and
   * the list came back empty". The first needs no records; the second is a claim
   * that needs checking, and conflating them is how a tab disappears unnoticed.
   */
  tabs: readonly TabRecord[] | null;
  /**
   * The geometry these tabs were measured against.
   *
   * Used only to notice that the geometry moved on. Kept here rather than
   * recomputed because the bundle builder cannot measure anything — that is the
   * kernel's job — so it can only ask whether two things still agree.
   */
  tabsAgainst: Shapes | null;
}

export interface EvaluationBundle {
  composition: {
    layers: {
      index: number;
      name: string;
      geometry: Shapes;
      bridges: readonly TabRecord[];
    }[];
  };
}

/** Why a bundle could not be built. Never empty: there is no such thing as "mostly fine". */
export interface BundleProblem {
  /** The layer the bundle is wrong about, by index. */
  layer: number;
  layerName: string;
  message: string;
}

export type BundleResult =
  | { ok: true; bundle: EvaluationBundle }
  | { ok: false; problem: BundleProblem };

/** Identity for a shape set — see `repair.ts` for why this is not the kernel hash. */
export function sameGeometry(a: Shapes, b: Shapes): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

/**
 * Assemble the evaluation bundle, or say why it cannot be assembled.
 *
 * Layer order is the document's order and nothing is sorted or deduplicated: the
 * browser entry takes no `application_order` and the validator falls back to
 * `index`, so the paint order is exactly the order the editor shows. Reordering
 * here would silently change which layer T-12 considers painted first.
 */
export function buildEvaluationBundle(layers: readonly BundleLayer[]): BundleResult {
  if (layers.length === 0) {
    return {
      ok: false,
      problem: {
        layer: 0,
        layerName: "",
        message: "Нет слоёв для проверки.",
      },
    };
  }

  const out: EvaluationBundle["composition"]["layers"] = [];

  for (const layer of layers) {
    if (layer.tabs !== null) {
      if (layer.tabsAgainst === null || !sameGeometry(layer.tabsAgainst, layer.shapes)) {
        return {
          ok: false,
          problem: {
            layer: layer.index,
            layerName: layer.name,
            message:
              `Геометрия слоя «${layer.name}» изменилась после постановки язычков. ` +
              "Записи о язычках больше не описывают эту геометрию, поэтому T-06 и " +
              "T-07 проверили бы утверждение, а не маску. Перепоставьте язычки.",
          },
        };
      }
    }

    out.push({
      index: layer.index,
      name: layer.name,
      geometry: layer.shapes,
      // A layer that was never bridged declares an empty list, which is what the
      // kernel reads as "no tabs to check". The unanchored-island rule still sees
      // the island itself, so an unbridged layer is reported for the reason it is
      // actually wrong rather than being passed over.
      bridges: layer.tabs ?? [],
    });
  }

  return { ok: true, bundle: { composition: { layers: out } } };
}

/** What to show for a verdict, in the order the user has to act on it. */
export interface VerdictView {
  /** The four documented verdicts, plus the refusal above. */
  kind: "verdict" | "refused";
  headline: string;
  /** Violations and layout notes, each a line the validator wrote. */
  messages: readonly string[];
  /** Layers that had to be laid out before the verdict meant anything. */
  nestedLayers: readonly number[];
  /** False when the numbers came from a profile no physical cut has confirmed. */
  limitsVerified: boolean;
}

const HEADLINES: Record<string, string> = {
  PASS: "Годно к вырезанию",
  PASS_WITH_WARNINGS: "Годно с замечаниями",
  FAIL: "Не годно",
  FATAL: "Проверка не выполнена",
};

/**
 * Turn a verdict into what the user reads.
 *
 * The headline is keyed by verdict name rather than by a boolean, because a
 * `FATAL` is not a failing project — it is a check that did not run, and showing
 * it in red as "not manufacturable" would tell the user their mask is broken when
 * the truth is that nothing was measured.
 *
 * `limits_physically_verified` is carried rather than assumed: a threshold taken
 * from a datasheet is not a threshold a cut has confirmed, and the editor saying
 * "PASS" without that caveat is the project's own claim, not the material's.
 */
export function verdictView(
  verdict: string,
  messages: readonly string[],
  nestedLayers: readonly number[],
  limitsVerified: boolean
): VerdictView {
  const headline = HEADLINES[verdict];
  return {
    kind: "verdict",
    headline: headline ?? `Неизвестный вердикт: ${verdict}`,
    messages,
    nestedLayers,
    limitsVerified,
  };
}

/** The refusal shown when no bundle could be built. */
export function refusedView(problem: BundleProblem): VerdictView {
  return {
    kind: "refused",
    headline: "Проверка не выполнена",
    messages: [problem.message],
    nestedLayers: [],
    limitsVerified: false,
  };
}