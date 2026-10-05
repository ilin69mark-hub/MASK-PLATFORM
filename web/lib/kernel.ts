// @ts-check
/**
 * Thin TypeScript facade over the WASM kernel.
 *
 * Every geometry decision is made in Rust. This module exists to load the
 * artifact and to give the UI typed arguments — not to reimplement anything. A
 * measurement taken here must equal the server's, or the editor will show a
 * clean project that the server then rejects.
 *
 * The shapes below mirror what `engine/wasm/src/lib.rs` serialises. They are
 * asserted against the kernel by `engine/wasm/scripts/runtime_check.mjs`.
 */

export type Pt = [number, number];

export interface Contour {
  points: Pt[];
  closed: boolean;
}

export interface Shape {
  outer: Contour;
  holes: Contour[];
}

export interface Shapes {
  shapes: Shape[];
}

export interface LayerResult {
  area_mm2: number;
  min_width_mm: number | null;
  /** Absent rather than null: see the note in the Rust struct. */
  island_count: number;
  hole_count: number;
  bbox: [number, number, number, number];
  content_hash: string;
  /** Canonical SVG path data, ready to draw without further processing. */
  path_data: string[];
}

export interface ValidationResult {
  verdict: "PASS" | "PASS_WITH_WARNINGS" | "FAIL" | "FATAL";
  error_count: number;
  warning_count: number;
  info_count: number;
  messages: string[];
  limits_physically_verified: boolean;
}

export interface EngineConstants {
  canon_quantum_mm: number;
  coord_dp: number;
  measure_report_dp: number;
  min_width_eps_mm: number;
  max_coordinate_mm: number;
  lib_scale: number;
  lib_math_mode: "Integer" | "Float";
  engine_version: string;
}

export interface EffectiveLimits {
  line_width_mm: number;
  gap_mm: number;
  island_width_mm: number;
  corner_radius_mm: number;
  bridge_width_mm: number;
  max_features_per_100cm2: number;
  max_area_mm2: number;
  max_tile_width_mm: number;
  max_tile_length_mm: number;
  kerf_mm: number;
  registration_error_mm: number;
}

/** The subset of the WASM module this UI depends on. */
interface Kernel {
  engine_version(): string;
  engine_constants(): EngineConstants;
  effective_limits(material: string, plotter: string): EffectiveLimits;
  measure_layer(shapes: string): LayerResult;
  union(a: string, b: string): Shapes;
  widen_centreline(points: string, width: number): LayerResult;
  content_hash(shapes: string): string;
  validate_project(
    project: string,
    material: string,
    plotter: string
  ): ValidationResult;
}

let kernel: Kernel | null = null;
let loading: Promise<Kernel> | null = null;

/**
 * Default loader: fetch the artifact `make wasm-dist` produces.
 *
 * `wasm-pack --target web` emits an ES module whose default export is an
 * initialiser. Whether a bundler hands back that module directly, or wrapped,
 * is a detail of the bundler — so both shapes are accepted and only the
 * presence of `measure_layer` is treated as proof of a working kernel.
 */
const DEFAULT_URL = "/pkg/mask_wasm.js";

async function defaultLoader(): Promise<unknown> {
  const mod = (await import(/* @vite-ignore */ DEFAULT_URL)) as Record<
    string,
    unknown
  >;
  const init = mod.default;
  if (typeof init === "function") {
    await (init as () => Promise<void>)();
  }
  return mod;
}

/**
 * Load the kernel once. Repeated calls share the same instance.
 *
 * The loader is injectable so tests can supply a stub and so the artifact can
 * be served from a different path without the UI caring.
 */
export async function loadKernel(
  loader: () => Promise<unknown> = defaultLoader
): Promise<Kernel> {
  if (kernel) return kernel;
  if (loading) return loading;

  loading = (async () => {
    const candidate = await loader();

    // Accepted shapes: the module itself, or a namespace wrapping it.
    const resolved =
      candidate && typeof candidate === "object" && "measure_layer" in candidate
        ? (candidate as Kernel)
        : ((candidate as { default?: Kernel }).default as Kernel | undefined);

    if (typeof resolved?.measure_layer !== "function") {
      throw new Error(
        "Mask kernel loaded but exposes no measure_layer(); check that " +
          `${DEFAULT_URL} was built with \`make wasm-dist\``
      );
    }
    kernel = resolved;
    return resolved;
  })();

  return loading;
}

/** Test seam: install a kernel directly. */
export function __setKernelForTests(stub: Kernel | null): void {
  kernel = stub;
  loading = null;
}

export function isLoaded(): boolean {
  return kernel !== null;
}

// ── geometry helpers ──────────────────────────────────────────────────

/** Counter-clockwise ring: positive signed area, so it can serve as an outer contour. */
export function rect(x: number, y: number, w: number, h: number): Contour {
  return {
    points: [
      [x, y],
      [x + w, y],
      [x + w, y + h],
      [x, y + h],
    ],
    closed: true,
  };
}

/**
 * Clockwise ring: negative signed area, so it can serve as a hole.
 *
 * Orientation is not a convention here, it is an invariant. Signed area sums
 * outer contours against hole contours, so a hole wound the same way as its outer
 * adds instead of subtracting. The kernel rejects that input; building a hole with
 * `rect` instead of this helper is the mistake that would trigger it.
 */
export function rectCW(x: number, y: number, w: number, h: number): Contour {
  return {
    points: [
      [x, y],
      [x, y + h],
      [x + w, y + h],
      [x + w, y],
    ],
    closed: true,
  };
}

export function shapeOf(outer: Contour, holes: Contour[] = []): Shape {
  return { outer, holes };
}

export function toShapes(...shapes: Shape[]): Shapes {
  return { shapes };
}

/**
 * Build path data for drawing without going through the kernel.
 *
 * Only used for geometry the UI has just constructed and has not yet measured —
 * anything the kernel has seen comes back canonical from `measure_layer`.
 */
export function pathDataOf(shapes: Shapes, dp = 4): string[] {
  const fmt = (v: number) => v.toFixed(dp);
  const emit = (c: Contour): string => {
    const first = c.points[0];
    if (!first) return "";
    const head = `M ${fmt(first[0])} ${fmt(first[1])}`;
    const body = c.points
      .slice(1)
      .map((p) => `L ${fmt(p[0])} ${fmt(p[1])}`)
      .join(" ");
    return `${head}${body ? ` ${body}` : ""}${c.closed ? " Z" : ""}`;
  };
  return shapes.shapes
    .flatMap((s) => [emit(s.outer), ...s.holes.map(emit)])
    .filter((d) => d.length > 0);
}

// ── kernel wrappers ───────────────────────────────────────────────────

function require_(): Kernel {
  if (!kernel) throw new Error("Mask kernel not loaded — call loadKernel() first");
  return kernel;
}

export function constants(): EngineConstants {
  return require_().engine_constants();
}

export function version(): string {
  return require_().engine_version();
}

export function limits(materialJson: string, plotterJson: string): EffectiveLimits {
  return require_().effective_limits(materialJson, plotterJson);
}

export function measure(shapes: Shapes): LayerResult {
  return require_().measure_layer(JSON.stringify(shapes));
}

export function union(a: Shapes, b: Shapes): Shapes {
  return require_().union(JSON.stringify(a), JSON.stringify(b));
}

export function widen(points: Pt[], widthMm: number): LayerResult {
  return require_().widen_centreline(JSON.stringify(points), widthMm);
}

export function hash(shapes: Shapes): string {
  return require_().content_hash(JSON.stringify(shapes));
}

export function validate(
  project: unknown,
  materialJson: string,
  plotterJson: string
): ValidationResult {
  return require_().validate_project(
    JSON.stringify(project),
    materialJson,
    plotterJson
  );
}

/** Format a length the way the engine reports it, so the UI cannot disagree. */
export function reportMm(value: number, dp?: number): string {
  const k = constants();
  return `${value.toFixed(dp ?? k.measure_report_dp)} mm`;
}

/** Format the project identifier for display and for printed labels. */
export function projectLabel(id: string, revision: number): string {
  return `${id} rev ${revision}`;
}