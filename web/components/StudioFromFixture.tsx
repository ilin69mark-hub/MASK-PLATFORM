"use client";

import { useEffect, useState } from "react";

import { StudioCanvas } from "@/components/StudioCanvas";
import {
  type Contour,
  type Shapes,
  loadKernel,
  toShapes,
  union,
  widen,
} from "@/lib/kernel";

/**
 * Builds a layer's geometry in the browser, through the kernel.
 *
 * The stroke happens in WASM rather than in TypeScript because the stroke is
 * what decides vertex order and miter placement, and the content hash depends on
 * exactly those details. Reimplementing it here would give the editor different
 * geometry from the server, which is the failure mode ADR-0013 exists to
 * prevent.
 */

export interface FixtureLayerData {
  index: number;
  name: string;
  width_mm: number;
  centrelines: [number, number][][];
}

export interface StudioFromFixtureProps {
  projectId: string;
  revision: number;
  layers: FixtureLayerData[];
  minWidthMm: number;
  /** Minimum gap between two elements (T-02), from the same profile. */
  minGapMm: number;
}

/** Parse the kernel's canonical path data back into a contour. */
function contourFromPathData(d: string): Contour | null {
  const nums = d.match(/-?\d+(?:\.\d+)?/g);
  if (!nums || nums.length < 6) return null;
  const points: [number, number][] = [];
  for (let i = 0; i + 1 < nums.length; i += 2) {
    points.push([Number(nums[i]), Number(nums[i + 1])]);
  }
  return { points, closed: d.trim().endsWith("Z") };
}

/** Union two shape sets by asking the kernel. */
async function unionAll(shapeSets: Shapes[]): Promise<Shapes> {
  if (shapeSets.length === 0) return toShapes();
  let merged = shapeSets[0] as Shapes;
  for (let i = 1; i < shapeSets.length; i += 1) {
    merged = await union(merged, shapeSets[i] as Shapes);
  }
  return merged;
}

export function StudioFromFixture({
  projectId,
  revision,
  layers,
  minWidthMm,
  minGapMm,
}: StudioFromFixtureProps) {
  const [ready, setReady] = useState(false);
  const [shapesByLayer, setShapesByLayer] = useState<Shapes[]>([]);
  const [active, setActive] = useState(0);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    (async () => {
      try {
        await loadKernel();
        const built: Shapes[] = [];
        for (const layer of layers) {
          const bands: Shapes[] = [];
          for (const line of layer.centrelines) {
            const band = widen(line, layer.width_mm);
            const shape = band.path_data
              .map(contourFromPathData)
              .filter((c): c is Contour => c !== null);
            if (shape.length > 0) {
              bands.push(toShapes(...shape.map((c) => ({ outer: c, holes: [] }))));
            }
          }
          built.push(await unionAll(bands));
        }
        if (!cancelled) {
          setShapesByLayer(built);
          setReady(true);
        }
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [layers]);

  if (error) {
    return (
      <div role="alert" style={{ color: "#c00", padding: "1rem" }}>
        Ядро не загрузилось: {error}
      </div>
    );
  }

  if (!ready) {
    return <p style={{ padding: "1rem" }}>Загрузка геометрического ядра…</p>;
  }

  const current = shapesByLayer[active];

  return (
    <div>
      <nav
        aria-label="Слои"
        style={{ display: "flex", gap: "0.25rem", padding: "0.5rem 1.5rem", flexWrap: "wrap" }}
      >
        {layers.map((layer, i) => (
          <button
            key={layer.index}
            type="button"
            aria-pressed={i === active}
            onClick={() => setActive(i)}
            style={{
              padding: "0.35rem 0.7rem",
              border: "1px solid #ccc",
              background: i === active ? "#111" : "#fff",
              color: i === active ? "#fff" : "#111",
              cursor: "pointer",
              borderRadius: "4px",
            }}
          >
            {String(layer.index).padStart(2, "0")} {layer.name}
          </button>
        ))}
      </nav>

      {current && (
        <StudioCanvas
          shapes={current}
          projectId={projectId}
          revision={revision}
          minWidthMm={minWidthMm}
          minGapMm={minGapMm}
        />
      )}
    </div>
  );
}