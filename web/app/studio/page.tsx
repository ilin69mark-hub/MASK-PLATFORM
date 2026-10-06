import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";

import { StudioFromFixture, type FixtureLayerData } from "@/components/StudioFromFixture";

/**
 * Mask Studio page.
 *
 * Reads the same profile and fixture files the CLI uses. Pointing the studio at
 * a different material than production would be a silent way to validate a design
 * against thresholds that will not apply on the bench.
 *
 * Server component: it only reads files. All geometry work happens in the
 * browser through the WASM kernel, because the kernel needs no server and the
 * studio has to keep working without one.
 */

// This file lives at web/app/studio/page.tsx, so three levels up is the
// repository root where engine/ lives. Getting this wrong fails at build time
// with a bare ENOENT, which is a poor way to learn a path is off.
const ROOT = resolve(dirname(new URL(import.meta.url).pathname), "../../..");

const FIXTURE = resolve(ROOT, "engine/fixtures/proof_panel.json");
const MATERIAL = resolve(ROOT, "engine/profiles/data/matte-vinyl-090.v4.json");
const PLOTTER = resolve(ROOT, "engine/profiles/data/knife-25mm.v2.json");

interface Fixture {
  project_id: string;
  revision: number;
  layers: {
    index: number;
    name: string;
    width_mm: number;
    centrelines: [number, number][][];
  }[];
}

export default async function StudioPage() {
  const [fixtureText, materialJson, plotterJson] = await Promise.all([
    readFile(FIXTURE, "utf8"),
    readFile(MATERIAL, "utf8"),
    readFile(PLOTTER, "utf8"),
  ]);

  const fixture = JSON.parse(fixtureText) as Fixture;
  const material = JSON.parse(materialJson) as {
    code: string;
    version: number;
    minimums: { line_width_mm: number; gap_mm: number };
    status: string;
  };

  const layers: FixtureLayerData[] = fixture.layers.map((l) => ({
    index: l.index,
    name: l.name,
    width_mm: l.width_mm,
    centrelines: l.centrelines,
  }));

  return (
    <main style={{ fontFamily: "system-ui, sans-serif" }}>
      <StudioFromFixture
        projectId={fixture.project_id}
        revision={fixture.revision}
        layers={layers}
        materialJson={materialJson}
        plotterJson={plotterJson}
        minWidthMm={material.minimums.line_width_mm}
        minGapMm={material.minimums.gap_mm}
      />

      <section style={{ padding: "0 1.5rem 2rem", maxWidth: "52rem", color: "#444" }}>
        <h2 style={{ fontSize: "1rem" }}>Что это за версия</h2>
        <p style={{ lineHeight: 1.55 }}>
          Измерения на экране считает то же ядро на Rust, что и сервер, поэтому
          число совпадает с числом в производственном отчёте. Профиль:{" "}
          <code>
            {material.code} v{material.version}
          </code>{" "}
          (<strong>{material.status}</strong> — пороги взяты из datasheet и не
          подтверждены вырезом реального материала).
        </p>
        <p style={{ lineHeight: 1.55 }}>
          Скачивание даёт превью, а не производственный файл. Настоящие файлы
          выдаёт сервер: только так артефакты совпадают с манифестом и могут быть
          воспроизведены через год.
        </p>
        <p style={{ lineHeight: 1.55 }}>
          Не реализовано: инструменты редактирования вершин, слои перекрываются
          графом, замер зазора между элементами, трёхмерный просмотр.
        </p>
      </section>
    </main>
  );
}