import assert from "node:assert/strict";
import test from "node:test";

import {
  buildEvaluationBundle,
  refusedView,
  verdictView,
  type BundleLayer,
} from "../lib/editor/bundle.ts";
import type { Shapes, TabRecord } from "../lib/kernel.ts";

const FRAME: Shapes = {
  shapes: [{ outer: { points: [{ x: 0, y: 0 }, { x: 40, y: 0 }, { x: 40, y: 40 }, { x: 0, y: 40 }], closed: true }, holes: [] }],
};

const ISLAND: Shapes = {
  shapes: [
    { outer: { points: [{ x: 0, y: 0 }, { x: 40, y: 0 }, { x: 40, y: 40 }, { x: 0, y: 40 }], closed: true }, holes: [] },
    { outer: { points: [{ x: 10, y: 10 }, { x: 10, y: 14 }, { x: 14, y: 14 }, { x: 14, y: 10 }], closed: true }, holes: [] },
  ],
};

const TAB: TabRecord = { width_mm: 1.2, length_mm: 4.0, at: { x: 10, y: 10 } };

const LAYER = (over: Partial<BundleLayer> = {}): BundleLayer => ({
  index: 1,
  name: "панель",
  shapes: FRAME,
  tabs: null,
  tabsAgainst: null,
  ...over,
});

test("a layer that was never bridged declares no tabs and still validates", () => {
  // The empty list is not "no tabs to check because there are none" — it is the
  // honest reading of a layer nobody anchored. The unanchored-island rule still
  // sees the island, so the layer is reported for what is actually wrong with it.
  const result = buildEvaluationBundle([LAYER()]);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.deepEqual(result.bundle.composition.layers[0]?.bridges, []);
});

test("the tabs a layer was built with travel with its geometry", () => {
  // Without this, T-06 and T-07 have nothing to read and the validator passes a
  // mask with a free island in it.
  const result = buildEvaluationBundle([
    LAYER({ shapes: ISLAND, tabs: [TAB], tabsAgainst: ISLAND }),
  ]);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.deepEqual(result.bundle.composition.layers[0]?.bridges, [TAB]);
  assert.equal(result.bundle.composition.layers[0]?.geometry, ISLAND);
});

test("layer order is kept exactly as given, because paint order is index order", () => {
  // The browser entry takes no `application_order`, so the validator falls back
  // to `index`. Sorting here would silently change which layer T-12 treats as
  // painted first, and nothing downstream would report the change.
  const result = buildEvaluationBundle([
    LAYER({ index: 7, name: "сначала" }),
    LAYER({ index: 2, name: "потом" }),
  ]);
  assert.equal(result.ok, true);
  if (!result.ok) return;
  assert.deepEqual(
    result.bundle.composition.layers.map((l) => [l.index, l.name]),
    [
      [7, "сначала"],
      [2, "потом"],
    ]
  );
});

test("records that no longer describe the geometry stop the check rather than pass it", () => {
  // The whole failure this guards: drag a vertex after bridging, and the tabs are
  // still claimed at positions the geometry no longer has. Shipping the records
  // has the validator check a claim; dropping them checks nothing.
  const result = buildEvaluationBundle([
    LAYER({ shapes: ISLAND, tabs: [TAB], tabsAgainst: FRAME }),
  ]);
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.problem.layer, 1);
  assert.equal(result.problem.layerName, "панель");
  assert.match(result.problem.message, /перепоставьте язычки/i);
  assert.match(result.problem.message, /T-06/);
});

test("records with nothing to compare against are treated as stale, not trusted", () => {
  // `tabs` without `tabsAgainst` cannot be shown to describe anything, so it is
  // the same refusal as a mismatch — not a licence to ship the claim.
  const result = buildEvaluationBundle([LAYER({ tabs: [TAB], tabsAgainst: null })]);
  assert.equal(result.ok, false);
});

test("no layers is a refusal, not a passing project", () => {
  // An empty bundle would validate trivially, and "nothing is wrong" is not a
  // statement anyone should be able to obtain by deleting their layers.
  const result = buildEvaluationBundle([]);
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.match(result.problem.message, /нет слоёв/i);
});

test("a refusal names the layer it is about", () => {
  // A user with thirteen layers cannot act on "the geometry changed".
  const result = buildEvaluationBundle([
    LAYER({ index: 0, name: "первый" }),
    LAYER({ index: 4, name: "четвёртый", shapes: ISLAND, tabs: [TAB], tabsAgainst: FRAME }),
    LAYER({ index: 9, name: "девятый" }),
  ]);
  assert.equal(result.ok, false);
  if (result.ok) return;
  assert.equal(result.problem.layer, 4);
  assert.ok(result.problem.message.includes("четвёртый"));
});

test("each verdict has its own headline, and an unknown one says so", () => {
  assert.equal(verdictView("PASS", [], [], true).headline, "Годно к вырезанию");
  assert.equal(verdictView("FAIL", [], [], true).headline, "Не годно");
  const unknown = verdictView("MAYBE", [], [], true);
  assert.match(unknown.headline, /Неизвестный вердикт/);
  assert.ok(unknown.headline.includes("MAYBE"));
});

test("FATAL is a check that did not run, not a mask that is broken", () => {
  // Both are non-green, and conflating them tells the user their mask is
  // unmanufacturable when the truth is that nothing was measured.
  const fatal = verdictView("FATAL", ["не удалось прочитать профиль"], [], false);
  assert.equal(fatal.headline, "Проверка не выполнена");
  assert.notEqual(fatal.headline, verdictView("FAIL", [], [], false).headline);
  assert.deepEqual(fatal.messages, ["не удалось прочитать профиль"]);
});

test("the verdict carries the messages and the layers a layout had to move", () => {
  const view = verdictView("PASS_WITH_WARNINGS", ["слой 2 разложен"], [2], false);
  assert.deepEqual(view.messages, ["слой 2 разложен"]);
  assert.deepEqual(view.nestedLayers, [2]);
  assert.equal(view.limitsVerified, false);
});

test("a refusal reads as a refusal and carries the reason", () => {
  const view = refusedView({ layer: 1, layerName: "панель", message: "причина" });
  assert.equal(view.kind, "refused");
  assert.deepEqual(view.messages, ["причина"]);
  assert.deepEqual(view.nestedLayers, []);
  assert.equal(view.limitsVerified, false);
});