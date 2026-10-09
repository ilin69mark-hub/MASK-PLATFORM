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

export interface GapReport {
  /** Index of the first region. */
  a: number;
  /** Index of the second region. */
  b: number;
  /** Distance in millimetres. Zero when the regions touch or overlap. */
  mm: number;
}

export interface Crossing {
  shape: number;
  /** 0 is the outer contour, 1.. are holes. */
  contour: number;
  segment: number;
  at: [number, number];
}

export interface LayerResult {
  area_mm2: number;
  /**
   * T-02. Undefined for a single region: it has no neighbour, so a number here
   * would be invented rather than measured.
   *
   * `undefined` and not `null`, because that is what arrives. An absent Rust
   * `Option` crosses the boundary as `undefined`, so a facade promising `null`
   * would type-check against a value that never occurs — and would quietly
   * invite `=== null`, which is false for `undefined`, into code that decides
   * whether a rule passed.
   */
  min_width_mm: number | undefined;
  /** T-02. See `min_width_mm` on why absence rather than null. */
  min_gap: GapReport | undefined;
  /** T-08. Non-empty means the geometry is not what it appears to be. */
  self_intersections: Crossing[];
  /** Absent rather than null: see the note in the Rust struct. */
  island_count: number;
  hole_count: number;
  /**
   * `[minX, minY, width, height]` in millimetres — the same four numbers and the
   * same order the validator publishes.
   */
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
  /**
   * Layers that were laid out because they did not fit the field.
   *
   * Empty for {@link validate}. A verdict that differs between the two calls
   * differs because of this list, so a caller comparing them must read it rather
   * than assume the geometry was the same.
   */
  nested_layers: number[];
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

/**
 * The effective limits, exactly as `rules::EffectiveLimits` publishes them.
 *
 * Eight of these were missing here while the kernel returned them all. That is
 * harmless at runtime — TypeScript only complains about a field it wants and
 * finds absent — and it is why the gap went unnoticed: nothing fails, the facade
 * just quietly denies the editor numbers the engine has. `kernel-contract-check`
 * now asserts every field is present, so the two cannot drift again.
 */
export interface EffectiveLimits {
  line_width_mm: number;
  gap_mm: number;
  island_width_mm: number;
  island_area_mm2: number;
  feature_length_mm: number;
  /** Longest cuttable tab, from `material.minimums.bridge_length_mm` (T-07). */
  bridge_length_mm: number;
  corner_radius_mm: number;
  /** Chord tolerance for flattening curves, carried to the blade doing the cut. */
  curve_tolerance_mm: number;
  min_angle_deg: number;
  bridge_width_mm: number;
  max_features_per_100cm2: number;
  max_area_mm2: number;
  max_tile_width_mm: number;
  max_tile_length_mm: number;
  kerf_mm: number;
  registration_error_mm: number;
  /** What the material tolerates, from `physical.tolerance_registration_mm`. */
  registration_budget_mm: number;
  peel_angle_min_deg: number;
  /**
   * Free text, deliberately: a new material may declare a peel rate this build
   * has never heard of, and rejecting the profile would be worse than carrying
   * the string through.
   */
  peel_rate: string;
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
  nest_and_validate_project(
    project: string,
    material: string,
    plotter: string
  ): ValidationResult;
  move_vertex(shapes: string, edit: VertexEdit): EditResult;
  widen_shape_set(shapes: string, target_width_mm: number): EditResult;
  bridge_unanchored_islands(shapes: string, width_mm: number, max_length_mm: number): RepairResult;
}

/**
 * What one edit produced: canonical geometry to keep, and its measurement.
 *
 * Both are needed. The shapes go back into state so the next edit addresses the
 * same vertex indices; the measurement comes with them so the numbers on screen
 * belong to the geometry that will be validated.
 */
export interface EditResult {
  shapes: Shapes;
  measured: LayerResult;
}

/** One vertex move: which point, and where it goes. */
export interface VertexEdit {
  /** Index into `Shapes.shapes`. */
  shape: number;
  /** 0 for the outer contour, 1.. for its holes — `Shape::contours()` order. */
  contour: number;
  /** Index into the contour's points. */
  vertex: number;
  x_mm: number;
  y_mm: number;
}

/** The result of an auto-fix pass: repaired geometry plus what was done to it. */
export interface RepairResult {
  /** Geometry with the tabs merged in, ready to draw and to re-validate. */
  shapes: Shapes;
  /// The tabs that were added, in the form the validator checks.
  bridges: ReadonlyArray<{ width_mm: number; length_mm: number; at: [number, number] }>;
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

/**
 * Move one vertex, through the kernel.
 *
 * The kernel applies the move, puts the coordinate on the canonical grid and
 * refuses a move that would collapse the contour, then measures the result. Doing
 * any of that in TypeScript would make the drawn geometry and the measured
 * geometry two different things — which is how an editor ends up certifying a
 * mask the station rejects.
 */
export function moveVertex(shapes: Shapes, edit: VertexEdit): EditResult {
  return require_().move_vertex(JSON.stringify(shapes), edit);
}

/** Grow every region that is narrower than `target_width_mm`.
 *
 * Both walls move outward by half the shortfall, so the total width grows
 * by the full difference. Regions already at or above the target are left
 * alone rather than shrunk.
 */
export function widenShapes(shapes: Shapes, target_width_mm: number): EditResult {
  return require_().widen_shape_set(JSON.stringify(shapes), target_width_mm);
}

/** Move a tab between an island and the surrounding film.
 *
 * This is the browser half of `maskctl --bridge`. Without it the editor cannot
 * place tabs, and without tabs the validator cannot check T-06/T-07.
 */
export function bridgeUnanchored(shapes: Shapes, width_mm: number, max_length_mm: number): RepairResult {
  return require_().bridge_unanchored_islands(JSON.stringify(shapes), width_mm, max_length_mm);
}

/**
 * Validate a project the way it will be printed: lay out every layer that does
 * not fit, then check the layout.
 *
 * This is the browser's counterpart to `maskctl --nest`, and the only way the
 * registration check (T-15) can see geometry — "where a layer starts" exists
 * only after a layout. The layout is computed by the kernel, never read from the
 * document, so a project cannot talk its way into passing.
 */
export function nestAndValidate(
  project: unknown,
  materialJson: string,
  plotterJson: string
): ValidationResult {
  return require_().nest_and_validate_project(
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