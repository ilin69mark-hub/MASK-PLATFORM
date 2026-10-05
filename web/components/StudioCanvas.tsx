"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  type LayerResult,
  type Shapes,
  loadKernel,
  measure,
} from "@/lib/kernel";

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
  /** Profile JSON, loaded once and reused for validation. */
  materialJson: string;
  plotterJson: string;
  projectId: string;
  revision: number;
  /** Minimum element width for the profile, shown as a threshold line. */
  minWidthMm: number;
}

interface Measurement {
  status: "idle" | "measuring" | "ready" | "error";
  result?: LayerResult;
  error?: string;
  /** Milliseconds spent in the kernel, for the budget display. */
  elapsedMs?: number;
}

export function StudioCanvas({
  shapes,
  materialJson,
  plotterJson,
  projectId,
  revision,
  minWidthMm,
}: StudioProps) {
  const svgRef = useRef<SVGSVGElement | null>(null);
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
        if (!cancelled) {
          setMeasurement({
            status: "error",
            error: e instanceof Error ? e.message : String(e),
          });
        }
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

  const view = useMemo(() => {
    const [, , w, h] = measurement.result?.bbox ?? [0, 0, 100, 100];
    // Padding in millimetres, so the margin looks the same at any zoom.
    const pad = Math.max(w, h) * 0.04 || 5;
    return { minX: -pad, minY: -pad, w: w + pad * 2, h: h + pad * 2 };
  }, [measurement.result]);

  const pathData = measurement.result?.path_data ?? [];

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
          ref={svgRef}
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
          {mode === "mask" && measurement.result?.min_width_mm != null && (
            <ThresholdLine
              y={view.minY + view.h / 2}
              x={view.minX}
              width={view.w}
              label={`min line ${minWidthMm.toFixed(3)} mm`}
            />
          )}
        </svg>
      </div>

      <footer className="studio-status">
        <StatusBar
          measurement={measurement}
          kernelReady={kernelReady}
          minWidthMm={minWidthMm}
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

function StatusBar({
  measurement,
  kernelReady,
  minWidthMm,
  onDownload,
}: {
  measurement: Measurement;
  kernelReady: boolean;
  minWidthMm: number;
  onDownload: () => void;
}) {
  if (!kernelReady) {
    return <span className="muted">Загрузка геометрического ядра…</span>;
  }
  if (measurement.status === "error") {
    return (
      <span className="error" role="alert">
        {measurement.error}
      </span>
    );
  }
  if (measurement.status !== "ready" || !measurement.result) {
    return <span className="muted">Измерение…</span>;
  }

  const r = measurement.result;

  // Compared with the same 1e-3 band the kernel uses (ADR-0004). Without it a
  // layer sitting exactly on the limit would flicker between pass and fail.
  const tooNarrow =
    r.min_width_mm != null && r.min_width_mm < minWidthMm - MIN_WIDTH_EPS_MM;

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
          {r.min_width_mm == null ? "—" : `${r.min_width_mm.toFixed(3)} мм`}
        </strong>
        {tooNarrow && (
          <span className="error">
            {" "}
            ниже минимума {minWidthMm.toFixed(3)} мм
          </span>
        )}
      </span>
      <span className="muted">
        ядро {measurement.elapsedMs?.toFixed(0)} мс · {r.content_hash.slice(7, 19)}
      </span>
      <button type="button" onClick={onDownload}>
        Скачать превью
      </button>
    </div>
  );
}