// Register the test-only TSX loader before any test module is imported.
// Static imports are resolved before module bodies run, so this registration
// cannot live inside a `.test.ts` file: by then Node has already tried and
// failed to parse the imported `.tsx`.
import { register } from "node:module";

register("./tsx-hooks.mjs", import.meta.url);
