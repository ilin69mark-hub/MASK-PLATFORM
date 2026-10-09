"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  type Crossing,
  type LayerResult,
  type Shapes,
  type VertexEdit,
  loadKernel,
  measure,
} from "@/lib/kernel";
import {
  type DrawnSizePx,
  type Point2D,
  pxRadiusToMm,
  pxToMm,
  viewFor,
} from "@/lib/editor/viewTransform";
import {
  beginDrag,
  editFor,
  idleDrag,
  moveDrag,
  shouldCommit,
  type DragState,
} from "@/lib/editor/dragEdit";
import { hitVertex, nearestVertexMm } from "@/lib/editor/hitTest";
import type { VerdictView } from "@/lib/editor/bundle";
import type { RepairOutcome } from "@/lib/editor/repair";
import { NodeTool } from "@/components/NodeTool";
import {
  type MeasurementState,
  belowMinimum,
  shownMm,
  statusView,
} from "@/lib/status";

/**
 * Mask Studio canvas.
 *
 * Every number shown here comes from the kernel. The component measures through
 * WASM and renders the path data the kernel returns, so what the user sees on
 * the canvas is the geometry that would be cut — not a re-derivation that might
 * disagree with it.
 */

export type ViewMode = "mask" | "result" | "nesting";

/**
 * Comparison band, matching the kernel's `MIN_WIDTH_EPS_MM` (ADR-0004).
 *
 * Duplicated deliberately rather than read from the kernel at module load: this
 * constant is needed during the first render, before the kernel has finished
 * loading, and a comparison that silently changed once the kernel arrived would
 * be worse than a documented duplicate. `kernel-contract-check.mjs` asserts the
 * two agree.
 */
const MIN_WIDTH_EPS_MM = 1e-3;

export interface StudioProps {
  /** Geometry of the selected layer, in millimetres. */
  shapes: Shapes;
  projectId: string;
  revision: number;
  /** Minimum element width for the profile, shown as a threshold line. */
  minWidthMm: number;
  /** Minimum gap between two elements (T-02), from the same profile. */
  minGapMm: number;
  /**
   * True once the layer was edited: it no longer derives from centrelines and
   * is a `regions`-layer (ADR-0014) rather than a widened fixture layer.
   */
  edited?: boolean;
  /** Last refused edit, if any — a kernel refusal, not a crash. */
  editError?: string | null;
  canUndo?: boolean;
  canRedo?: boolean;
  /**
   * False until the kernel has derived the profile's limits.
   *
   * The repairs stay disabled rather than guessing: a repair cut against its own
   * default tab width would place steel the bench will not cut, and the user
   * would find out at the station.
   */
  canRepair?: boolean;
  /** What the last repair did, shown so a change is never silent. */
  repair?: RepairOutcome | null;
  /**
   * What the validator said, or `null` when it has not been asked yet.
   *
   * Distinct from "no problems": the editor used to show a layer's measurements
   * and nothing else, so a mask on screen was indistinguishable from a mask that
   * had been checked.
   */
  verdict?: VerdictView | null;
  /**
   * Switch the Node tool on from outside. Left out, the canvas owns the toggle.
   *
   * It exists so the tool's two visible effects can be tested without a pointer:
   * whether the toggle announces that it is on, and whether the grab dots are
   * drawn at all. Both were wrong in ways no existing test could see, because
   * both are properties of a state the component held privately and showed
   * nothing about.
   */
  nodeToolOn?: boolean;
  /** One vertex move, applied by the owner through the kernel. */
  onEdit?: (edit: VertexEdit) => void;
  /** A drag began: the owner snapshots the geometry for undo. */
  onDragBegin?: () => void;
  /** The drag ended: the owner clears the snapshot. */
  onDragEnd?: () => void;
  onUndo?: () => void;
  onRedo?: () => void;
  /** Grow every narrow region to the material's minimum width. */
  onWiden?: () => void;
  /** Anchor every free island with a tab. */
  onBridge?: () => void;
  /** Validate as-is, with no layout. */
  onVerify?: () => void;
  /** Validate the way it would be printed: lay out what does not fit, then check. */
  onVerifyNested?: () => void;
}

/**
 * One definition of the measurement state, not two.
 *
 * This used to be a single `status` field with every other field optional, so
 * `{status: "error", result: someResult}` type-checked — a state that is both
 * measured and failed. The union in `lib/status.ts` makes that unrepresentable, and
 * the extractor was worth doing for this alone on top of being testable.
 */
type Measurement = MeasurementState<LayerResult>;

export function StudioCanvas({
  shapes,
  projectId,
  revision,
  minWidthMm,
  minGapMm,
  edited = false,
  editError = null,
  canUndo = false,
  canRedo = false,
  canRepair = false,
  repair = null,
  verdict = null,
  nodeToolOn,
  onEdit,
  onDragBegin,
  onDragEnd,
  onUndo,
  onRedo,
  onWiden,
  onBridge,
  onVerify,
  onVerifyNested,
}: StudioProps) {
  const [mode, setMode] = useState<ViewMode>("mask");
  const [kernelReady, setKernelReady] = useState(false);
  const [measurement, setMeasurement] = useState<Measurement>({ status: "idle" });

  useEffect(() => {
    let cancelled = false;
    loadKernel()
      .then(() => {
        if (!cancelled) setKernelReady(true);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        // `kernelReady` is set here as well as on success. It was not, and the
        // status bar checks `!kernelReady` before it checks for an error — so a
        // failed load left the editor reading "Загрузка геометрического ядра…"
        // forever, with the captured message unreachable behind it. The most likely
        // cause of that failure is a missing browser artifact, which is exactly the
        // state `make check` can leave you in: it never runs `wasm-dist`, so a green
        // check does not produce `/pkg/mask_wasm.js`.
        setKernelReady(true);
        setMeasurement({
          status: "error",
          error: e instanceof Error ? e.message : String(e),
        });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Measured through the kernel, debounced. A drag fires continuously and the
  // kernel is fast but not free, so measuring on every pointer move would make
  // dragging feel heavy on a large mask.
  useEffect(() => {
    if (!kernelReady) return;
    setMeasurement((prev) => ({ ...prev, status: "measuring" }));

    const handle = setTimeout(() => {
      const started = performance.now();
      try {
        const result = measure(shapes);
        setMeasurement({
          status: "ready",
          result,
          elapsedMs: performance.now() - started,
        });
      } catch (e) {
        setMeasurement({
          status: "error",
          error: e instanceof Error ? e.message : String(e),
        });
      }
    }, 300);

    return () => clearTimeout(handle);
  }, [shapes, kernelReady]);

  // Narrowed once, here, rather than at each use. The union says a result exists only
  // for a ready measurement; the old shape — `result?: LayerResult` beside a bare
  // `status` field — let a failed measurement carry a result too, which is how this
  // component ended up with an error it could not display.
  const ready = measurement.status === "ready" ? measurement.result : undefined;

  // The bounding box of the last measurement that produced one, kept across the
  // gap while a new one is on its way.
  //
  // It used to fall back to a placeholder whenever the measurement was not
  // `ready` — and every drag makes it not ready, because a committed edit
  // changes the geometry and the re-measurement is debounced. So the view
  // collapsed to a 100 mm box in the middle of the drag: the dots jumped
  // somewhere else on screen and every following pointer position was converted
  // through the wrong rectangle. The first edit landed, then the coordinate
  // mapping itself went wrong, which is indistinguishable from a dead tool.
  //
  // A ref rather than state on purpose: this is a cache of the last known value,
  // not something that schedules a render.
  const lastBbox = useRef<[number, number, number, number] | null>(null);
  if (ready?.bbox) lastBbox.current = ready.bbox;
  const bbox = ready?.bbox ?? lastBbox.current;

  // The first two numbers are where the geometry starts, not decoration. They
  // used to be discarded — the measurement reported `[0, 0, w, h]` whatever the
  // input — so the view was built around the origin and any geometry not at the
  // origin was drawn off the visible area.
  const view = useMemo(() => viewFor(bbox), [bbox]);

  // The Node tool: off until toggled (hotkey A). While on, vertices are
  // drawn and draggable, and each committed drag calls `onEdit`. Defined
  // after `view`, since the pixel↔mm mapping needs it.
  //
  // Owned here unless the owner switches it on through `nodeToolOn`, which is
  // how the tests reach a state that used to be reachable only by clicking.
  const [nodeToolOwned, setNodeTool] = useState(false);
  const nodeTool = nodeToolOn ?? nodeToolOwned;
  const [drawn, setDrawn] = useState<DrawnSizePx>({ width: 0, height: 0 });
  const svgRef = useRef<SVGSVGElement | null>(null);
  const [drag, setDrag] = useState<DragState>(idleDrag());
  const [pointerMm, setPointerMm] = useState<Point2D | null>(null);
  // Why a grab missed, in the editor's own millimetres. A pointer that lands 2 mm
  // from a vertex and one that lands 400 mm from it are the same silence, and
  // they are not the same fault.
  const [miss, setMiss] = useState<string | null>(null);
  // Live proof that the pointer reaches the canvas and that its position is
  // converted through the same rectangle the dots are drawn in. Without it, "the
  // tool does nothing" has no way to be told apart from "the pointer never
  // arrives", and those are different faults in different layers.
  const [pointerReadout, setPointerReadout] = useState<string | null>(null);
  const dragRef = useRef<DragState>(idleDrag());
  dragRef.current = drag;
  const lastMmRef = useRef<Point2D | null>(null);
  const nodeToolRef = useRef(nodeTool);
  nodeToolRef.current = nodeTool;

  // Measure the element the viewBox is drawn into, so pointer pixels
  // convert to millimetres against the real letterbox. The size is the
  // container's, not the viewBox's, and it changes with the window.
  useEffect(() => {
    const svg = svgRef.current;
    if (!svg) return;
    const measure = () => {
      const rect = svg.getBoundingClientRect();
      setDrawn({ width: rect.width, height: rect.height });
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(svg);
    return () => observer.disconnect();
  }, [kernelReady, view.minX, view.minY, view.w, view.h]);

  // Hotkey A toggles the Node tool. The tool is the only way to edit
  // geometry, so its shortcut is a single letter rather than a modifier
  // chord. Ignored while typing in a text field.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "a" && e.key !== "A") return;
      const target = e.target as HTMLElement | null;
      if (
        target &&
        (target.tagName === "INPUT" || target.tagName === "TEXTAREA")
      ) {
        return;
      }
      setNodeTool((v) => !v);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Pixel radius of the vertex grab target, turned into mm at the current
  // scale and letterbox factor.
  const grabRadiusMm = pxRadiusToMm(6, view, drawn);

  // Convert a pointer event to millimetres inside the viewBox, against the
  // element's real size and its letterbox.
  const toMm = useCallback(
    (clientX: number, clientY: number): Point2D | null => {
      const svg = svgRef.current;
      if (!svg) return null;
      const rect = svg.getBoundingClientRect();
      const box: DrawnSizePx = { width: rect.width, height: rect.height };
      return pxToMm(
        { x: clientX - rect.left, y: clientY - rect.top },
        view,
        box
      );
    },
    [view]
  );

  const handleNodePointerDown = useCallback(
    (e: React.PointerEvent<SVGSVGElement>) => {
      if (!nodeToolRef.current) return;
      const at = toMm(e.clientX, e.clientY);
      if (!at) return;
      const grabbed = hitVertex(shapes, at, { radiusMm: grabRadiusMm });
      const next = beginDrag(grabbed);
      setDrag(next);
      dragRef.current = next;
      lastMmRef.current = at;
      setPointerMm(at);
      setMiss(grabbed ? null : describeMiss(shapes, at, grabRadiusMm));
      if (grabbed) {
        onDragBegin?.();
        // Keep the moves coming to this element even off the vertices.
        e.currentTarget.setPointerCapture?.(e.pointerId);
      }
    },
    [shapes, grabRadiusMm, toMm, onDragBegin]
  );

  const handleNodePointerMove = useCallback(
    (e: React.PointerEvent<SVGSVGElement>) => {
      const at = toMm(e.clientX, e.clientY);
      if (!at) return;
      const state = dragRef.current;
      if (!state.grabbed) {
        lastMmRef.current = at;
        setPointerReadout(
          `указатель: ${at.x.toFixed(1)}; ${at.y.toFixed(1)} мм · захват ${grabRadiusMm.toFixed(3)} мм`
        );
        return;
      }
      setPointerMm(at);
      const from = lastMmRef.current ?? at;
      const next = moveDrag(state, from, at);
      lastMmRef.current = at;
      setDrag(next);
      dragRef.current = next;
      // Commit as the drag goes, so the measurement the user watches
      // belongs to the geometry being dragged. The history dedups the
      // repeated snapshots, so one drag is still one undo step.
      if (shouldCommit(next)) {
        const edit = editFor(next, at);
        if (edit) onEdit?.(edit);
      }
    },
    [toMm, onEdit, grabRadiusMm]
  );

  const endNodeDrag = useCallback(() => {
    if (dragRef.current.grabbed) onDragEnd?.();
    setDrag(idleDrag());
    dragRef.current = idleDrag();
    lastMmRef.current = null;
    setPointerMm(null);
  }, [onDragEnd]);

  const handleNodePointerUp = useCallback(
    (_e: React.PointerEvent<SVGSVGElement>) => {
      endNodeDrag();
    },
    [endNodeDrag]
  );

  // Release the drag when the pointer ends off-canvas, so a drag that
  // leaves the window does not leave the tool holding a vertex.
  useEffect(() => {
    if (!drag.grabbed) return;
    const leave = () => endNodeDrag();
    window.addEventListener("pointerup", leave);
    window.addEventListener("pointercancel", leave);
    return () => {
      window.removeEventListener("pointerup", leave);
      window.removeEventListener("pointercancel", leave);
    };
  }, [drag.grabbed, endNodeDrag]);

  const pathData = ready?.path_data ?? [];

  const handleDownload = useCallback(() => {
    // The browser produces a preview, not a production file. Production export
    // goes through the server so the artifacts are the same ones the manifest
    // records.
    const doc = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${view.minX} ${view.minY} ${view.w} ${view.h}">
  <rect width="100%" height="100%" fill="white"/>
  <g fill="none" stroke="black" stroke-width="0.15">
${pathData.map((d) => `    <path d="${d}"/>`).join("\n")}
  </g>
</svg>
`;
    const blob = new Blob([doc], { type: "image/svg+xml" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `${projectId}_rev${revision}_preview.svg`;
    a.click();
    URL.revokeObjectURL(url);
  }, [pathData, view, projectId, revision]);

  return (
    <div className="studio" data-testid="studio">
      <header className="studio-header">
        <h1>
          {projectId} <span className="rev">rev {revision}</span>
        </h1>
        <div className="modes" role="group" aria-label="Режим просмотра">
          {(["mask", "result", "nesting"] as const).map((m) => (
            <button
              key={m}
              type="button"
              aria-pressed={mode === m}
              onClick={() => setMode(m)}
            >
              {m === "mask" ? "Маска" : m === "result" ? "Результат" : "Раскладка"}
            </button>
          ))}
        </div>
      </header>

      {/*
        Gated on any editing affordance, not on `onUndo ?? onRedo` alone. It used
        to be the latter, which meant the repair buttons appeared only because
        the owner happened to wire undo — a component given a repair and nothing
        else rendered no toolbar at all, and the repairs silently did not exist.
      */}
      {(onUndo ?? onRedo ?? onEdit ?? onWiden ?? onBridge ?? onVerify ?? onVerifyNested) && (
        <div
          className="studio-editbar"
          role="toolbar"
          aria-label="Правки геометрии"
          style={{ display: "flex", gap: "0.5rem", padding: "0 1.5rem", alignItems: "center" }}
        >
<button
              type="button"
              onClick={() => setNodeTool((v) => !v)}
              aria-pressed={nodeTool}
              data-testid="node-tool-toggle"
              title="Перетаскивать вершины (клавиша A)"
            >
              Вершины
            </button>
            {onWiden && (
              <button
                type="button"
                onClick={onWiden}
                disabled={!canRepair}
                data-testid="widen-button"
                title={`Расширить узкие элементы до ${minWidthMm.toFixed(3)} мм`}
              >
                Расширить
              </button>
            )}
            {onBridge && (
              <button
                type="button"
                onClick={onBridge}
                disabled={!canRepair}
                data-testid="bridge-button"
                title="Привязать свободные островки язычками"
              >
                Язычки
              </button>
            )}
            {onVerify && (
              <button type="button" onClick={onVerify} data-testid="verify-button">
                Проверить
              </button>
            )}
            {onVerifyNested && (
              <button
                type="button"
                onClick={onVerifyNested}
                data-testid="verify-nested-button"
                title="Разложить слои, которые не помещаются, и проверить раскладку"
              >
                С раскладкой
              </button>
            )}
            {onUndo && (
              <button type="button" onClick={onUndo} disabled={!canUndo} aria-label="Отменить">
                Отменить
              </button>
            )}
          {onRedo && (
            <button type="button" onClick={onRedo} disabled={!canRedo} aria-label="Вернуть">
              Вернуть
            </button>
          )}
          {edited && (
            <span className="muted" data-testid="edited-badge">
              изменён вручную
            </span>
          )}
        </div>
      )}

      {verdict && <VerdictPanel view={verdict} />}

      {pointerReadout && nodeTool && (
        <p
          data-testid="pointer-readout"
          style={{ padding: "0 1.5rem", margin: "0.3rem 0 0", color: "#666", fontSize: "0.8rem" }}
        >
          {pointerReadout}
        </p>
      )}

      {miss && (
        <p
          role="status"
          data-testid="grab-miss"
          style={{ padding: "0 1.5rem", margin: "0.3rem 0 0", color: "#a15c00" }}
        >
          {miss}
        </p>
      )}

      {repair && (
        <p
          role="status"
          data-testid="repair-report"
          style={{ padding: "0 1.5rem", margin: 0, color: "#444" }}
        >
          {repair.message}
        </p>
      )}

      {editError && (
        <div role="alert" data-testid="edit-error" style={{ color: "#c00", padding: "0 1.5rem" }}>
          Правка отклонена: {editError}
        </div>
      )}

      <div className="studio-canvas">
        <svg
          ref={svgRef}
          viewBox={`${view.minX} ${view.minY} ${view.w} ${view.h}`}
          preserveAspectRatio="xMidYMid meet"
          role="img"
          aria-label="Геометрия слоя"
          onPointerDown={nodeTool ? handleNodePointerDown : undefined}
          onPointerMove={nodeTool ? handleNodePointerMove : undefined}
          onPointerUp={nodeTool ? handleNodePointerUp : undefined}
          style={nodeTool ? { touchAction: "none", cursor: "crosshair" } : undefined}
        >
          <rect
            x={view.minX}
            y={view.minY}
            width={view.w}
            height={view.h}
            fill="#fafafa"
            stroke="#ddd"
          />
          <g
            fill="none"
            stroke={mode === "result" ? "#c41820" : "#111"}
            strokeWidth={0.15}
            strokeLinejoin="round"
          >
            {pathData.map((d, i) => (
              <path key={i} d={d} />
            ))}
          </g>
          {/*
            Only while the tool is on. They used to render unconditionally, which
            was the second half of why the toggle looked broken: the dots appeared
            whether or not the Node tool was active, so their presence was taken as
            evidence that it was — and the tool then refused every drag. They are
            the tool's affordance, not the layer's, and an affordance that is
            visible when it does nothing is a lie about the editor's state.
          */}
          {nodeTool && (
            <NodeTool
              shapes={shapes}
              radiusMm={grabRadiusMm}
              grabbed={drag.grabbed}
              pointerMm={pointerMm}
            />
          )}
          {mode === "mask" && ready?.min_width_mm !== undefined && (
            <ThresholdLine
              y={view.minY + view.h / 2}
              x={view.minX}
              width={view.w}
              label={`min line ${minWidthMm.toFixed(3)} mm`}
            />
          )}
        </svg>
      </div>

      {ready && <CrossingWarning crossings={ready.self_intersections} />}

      <footer className="studio-status">
        <StatusBar
          measurement={measurement}
          kernelReady={kernelReady}
          minWidthMm={minWidthMm}
          minGapMm={minGapMm}
          onDownload={handleDownload}
        />
      </footer>
    </div>
  );
}

/**
 * What the validator said.
 *
 * `role="status"` rather than `alert`: a FAIL is an answer to a question the user
 * asked, not an emergency, and an alert would interrupt them mid-edit.
 *
 * The `limits_physically_verified` line is not decoration. A PASS means the mask
 * clears the thresholds the profile carries, and a profile whose thresholds came
 * from a datasheet has never been cut. Showing the green verdict without that
 * would let the editor speak for the material.
 */
function VerdictPanel({ view }: { view: VerdictView }) {
  const failed = view.kind === "refused" || view.headline === "Не годно";
  return (
    <section
      role="status"
      data-testid="verdict-panel"
      aria-label="Результат проверки"
      style={{ padding: "0 1.5rem", color: "#333" }}
    >
      <p style={{ margin: "0.4rem 0", fontWeight: 600 }}>
        {view.headline}
        {view.nestedLayers.length > 0 && (
          <span className="muted">
            {" "}
            · разложено слоёв: {view.nestedLayers.join(", ")}
          </span>
        )}
      </p>
      {!view.limitsVerified && (
        <p className="muted" data-testid="limits-unverified" style={{ margin: "0.2rem 0" }}>
          Пределы взяты из datasheet и не подтверждены вырезом реального материала.
        </p>
      )}
      {view.messages.length === 0 ? (
        <p className="muted" style={{ margin: "0.2rem 0" }}>
          Замечаний нет.
        </p>
      ) : (
        <ul style={{ margin: "0.2rem 0", paddingLeft: "1.2rem" }}>
          {view.messages.map((m, i) => (
            <li key={i} style={failed ? { color: "#c00" } : undefined}>
              {m}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/**
 * What to say when a grab found no vertex.
 *
 * The number is the whole point. "Nothing happened" is what a tool says when the
 * pointer is two millimetres off a vertex and also when it is four hundred — and
 * those are a tight radius and a broken coordinate mapping respectively. Saying
 * which one it was costs one line and turns an unreportable fault into one that
 * can be fixed.
 */
function describeMiss(
  shapes: Shapes,
  at: Point2D,
  radiusMm: number
): string {
  const nearest = nearestVertexMm(shapes, at);
  if (nearest === null) return "Здесь нет ни одной вершины.";
  if (nearest <= radiusMm * 1.5) {
    // Close enough that the radius is the reason, whatever the tie rule did.
    return (
      `Мимо: ближайшая вершина в ${nearest.toFixed(3)} мм, ` +
      `захват ${radiusMm.toFixed(3)} мм — наведите точнее.`
    );
  }
  return (
    `Мимо: ближайшая вершина в ${nearest.toFixed(1)} мм, ` +
    `захват ${radiusMm.toFixed(3)} мм. Это слишком далеко — указатель и точки ` +
    "считаются в разных координатах."
  );
}

/**
 * A reference line showing the profile's minimum element width.
 *
 * The canvas has no physical scale, so this is a scale bar rather than a
 * measurement overlay: it states the threshold in millimetres and draws a line
 * whose length is the threshold, which is what a user needs to judge whether a
 * feature is close to the limit.
 */
function ThresholdLine({
  y,
  x,
  width,
  label,
}: {
  y: number;
  x: number;
  width: number;
  label: string;
}) {
  const len = Math.min(width * 0.2, width);
  return (
    <g className="threshold" aria-hidden="true">
      <line
        x1={x + width / 2 - len / 2}
        x2={x + width / 2 + len / 2}
        y1={y}
        y2={y}
        stroke="#0a7"
        strokeWidth={0.3}
        strokeDasharray="2 1"
      />
      <text
        x={x + width / 2}
        y={y - 1}
        fill="#0a7"
        fontSize={Math.max(width * 0.015, 2)}
        textAnchor="middle"
      >
        {label}
      </text>
    </g>
  );
}

/**
 * The geometry is not what it appears to be.
 *
 * A contour that crosses itself has its lobes cancel in the signed area rule, so a
 * bow tie measures 0 mm² — a number, not an error. The CLI refuses such geometry
 * before any merge touches it; without this the editor showed a plausible area for
 * a shape the server would reject, and the user found out only at submission.
 *
 * Reported, never repaired: resolving the crossing reshapes the figure, and a mask
 * is a physical object cut out of film.
 */
function CrossingWarning({ crossings }: { crossings: Crossing[] }) {
  // The length is checked first, so `first` is present — but TypeScript cannot
  // see that through `length`, hence the assertion rather than a cast.
  const first = crossings[0];
  if (!first) return null;
  const contour =
    first.contour === 0 ? "внешний контур" : `дыра ${first.contour}`;
  return (
    <div className="studio-warn" role="alert" data-testid="crossing-warning">
      <strong>
        Контур пересекает сам себя{" "}
        {crossings.length === 1 ? "в одной точке" : `в ${crossings.length} точках`}.
      </strong>{" "}
      Первая — ({first.at[0].toFixed(3)}; {first.at[1].toFixed(3)}) мм, элемент{" "}
      {first.shape + 1}, {contour}. Площадь такой фигуры по знаковому правилу
      неверна: lobes взаимно сокращаются, и число выглядит как измерение, а не
      как ошибка. Объединение разрешит пересечение молча, поэтому вырежется не та
      фигура, что нарисована.
    </div>
  );
}

export function StatusBar({
  measurement,
  kernelReady,
  minWidthMm,
  minGapMm,
  onDownload,
}: {
  measurement: Measurement;
  kernelReady: boolean;
  minWidthMm: number;
  minGapMm: number;
  onDownload: () => void;
}) {
  // The decision of what to show lives in lib/status.ts, not here, because it was
  // once wrong in a way reading this file would not have caught. See the note there.
  const view = statusView(kernelReady, measurement);
  if (view.kind === "error") {
    return (
      <span className="error" role="alert">
        {view.text}
      </span>
    );
  }
  if (view.kind !== "metrics") {
    return <span className="muted">{view.text}</span>;
  }

  const r = view.result;

  // Compared with the same 1e-3 band the kernel uses (ADR-0004). Without it a
  // layer sitting exactly on the limit would flicker between pass and fail.
  const tooNarrow = belowMinimum(r.min_width_mm, minWidthMm, MIN_WIDTH_EPS_MM);
  // The same band for the gap (T-02). Two elements 0.900 mm apart with a 0.900 mm
  // minimum is manufacturable, and reporting it otherwise would train the user to
  // ignore this line.
  const tooClose = belowMinimum(r.min_gap?.mm, minGapMm, MIN_WIDTH_EPS_MM);

  return (
    <div className="status-row">
      <span>
        элементов: <strong>{r.island_count}</strong>
      </span>
      <span>
        отверстий: <strong>{r.hole_count}</strong>
      </span>
      <span>
        площадь: <strong>{r.area_mm2.toFixed(1)}</strong> мм²
      </span>
      <span>
        мин. ширина:{" "}
        <strong
          data-testid="min-width"
          className={tooNarrow ? "error" : undefined}
        >
          {shownMm(r.min_width_mm)}
        </strong>
        {tooNarrow && (
          <span className="error">
            {" "}
            ниже минимума {minWidthMm.toFixed(3)} мм
          </span>
        )}
      </span>
      <span>
        мин. зазор:{" "}
        <strong data-testid="min-gap" className={tooClose ? "error" : undefined}>
          {/* Undefined for one region: there is no neighbour to be close to, and
              printing 0.000 mm there would state that a gap was measured. */}
          {shownMm(r.min_gap?.mm)}
        </strong>
        {tooClose && (
          <span className="error">
            {" "}
            ниже минимума {minGapMm.toFixed(3)} мм
          </span>
        )}
      </span>
      <span className="muted">
        ядро {view.elapsedMs?.toFixed(0)} мс · {r.content_hash.slice(7, 19)}
      </span>
      <button type="button" onClick={onDownload}>
        Скачать превью
      </button>
    </div>
  );
}