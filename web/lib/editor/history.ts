/**
 * Undo and redo for geometry the user edited.
 *
 * Pure functions over immutable stacks, not a class or a React hook: the editor
 * holds geometry for several layers at once and switching layers must not make one
 * global stack undo a different layer's last move. The layer index is the key, and
 * the caller keeps one stack per layer.
 *
 * Snapshots are whole shape sets rather than inverse operations. An inverse of
 * "move a vertex" is a second vertex move, which would have to survive the kernel
 * canonicalising, rejecting or collapsing the geometry in between — and then the
 * undo would not be the shape that was there before, but the kernel's idea of it.
 * Keeping the geometry that actually existed can only reproduce it exactly.
 */

/** One undoable state. */
export interface Snapshot<T> {
  shapes: T;
}

/** A layer's edit history: what has been done, and what can be redone. */
export interface History<T> {
  /** States before each edit, oldest first. The current state is not stored. */
  past: Snapshot<T>[];
  /** States that were undone, oldest first. Redo pops from the end. */
  future: Snapshot<T>[];
}

export function emptyHistory<T>(): History<T> {
  return { past: [], future: [] };
}

/**
 * Record the state that existed before an edit.
 *
 * Calling this with the geometry that is already current is what a drag does on
 * every pointer move: the snapshot is taken once, at the start, and repeated calls
 * with the same shape are ignored. Otherwise a hundred mouse-moves would need a
 * hundred undos to get back to where the drag began.
 */
export function pushEdit<T>(
  history: History<T>,
  before: T,
  identity?: (value: T) => string
): History<T> {
  const key = identity ?? defaultIdentity;
  const current = history.past[history.past.length - 1];
  // The newest snapshot is the state before the last edit; if the drag has not
  // moved since then, pushing again would duplicate it.
  if (current && key(current.shapes) === key(before)) return history;
  return {
    past: [...history.past, { shapes: before }],
    future: [],
  };
}

/** The state to go back to, or nothing when there is nothing to undo. */
export function undo<T>(
  history: History<T>,
  current: T,
  identity?: (value: T) => string
): { history: History<T>; shapes: T } | null {
  const key = identity ?? defaultIdentity;
  if (history.past.length === 0) return null;
  const previous = history.past[history.past.length - 1];
  if (!previous) return null;
  if (key(previous.shapes) === key(current)) {
    // The current state is already the one being undone — a duplicate push.
    return {
      history: { past: history.past.slice(0, -1), future: history.future },
      shapes: previous.shapes,
    };
  }
  return {
    history: {
      past: history.past.slice(0, -1),
      future: [...history.future, { shapes: current }],
    },
    shapes: previous.shapes,
  };
}

/** The state to come back to, or nothing when there is nothing to redo. */
export function redo<T>(
  history: History<T>,
  current: T
): { history: History<T>; shapes: T } | null {
  if (history.future.length === 0) return null;
  const next = history.future[history.future.length - 1];
  if (!next) return null;
  return {
    history: { past: history.past, future: history.future.slice(0, -1) },
    shapes: next.shapes,
  };
}

export function canUndo<T>(history: History<T>): boolean {
  return history.past.length > 0;
}

export function canRedo<T>(history: History<T>): boolean {
  return history.future.length > 0;
}

/**
 * Identity by serialisation, used only when the caller supplies none.
 *
 * `content_hash` when the value has one, so two states that the kernel considers
 * identical are also identical here — otherwise a drag that ends where it started
 * would leave an undo step that undoes nothing visible.
 */
function defaultIdentity(value: unknown): string {
  const hashed = value as { content_hash?: string };
  if (hashed && typeof hashed.content_hash === "string") return hashed.content_hash;
  return JSON.stringify(value);
}