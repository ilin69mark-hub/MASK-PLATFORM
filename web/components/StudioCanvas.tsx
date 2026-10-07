"use client";

import { useCallback, useEffect, useMemo, useState } from "react";

import {
  type Crossing,
  type LayerResult,
  type Shapes,
  loadKernel,
  measure,
} from "@/lib/kernel";
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

  const view = useMemo(() => {
    const [, , w, h] = ready?.bbox ?? [0, 0, 100, 100];
    // Padding in millimetres, so the margin looks the same at any zoom.
    const pad = Math.max(w, h) * 0.04 || 5;
    return { minX: -pad, minY: -pad, w: w + pad * 2, h: h + pad * 2 };
  }, [ready]);

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

      <div className="studio-canvas">
        <svg
          viewBox={`${view.minX} ${view.minY} ${view.w} ${view.h}`}
          preserveAspectRatio="xMidYMid meet"
          role="img"
          aria-label="Геометрия слоя"
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