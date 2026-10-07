// Minimal TSX support for the Node test runner.
// Node's `--experimental-strip-types` removes types but does not transform JSX,
// so importing a `.tsx` component would otherwise fail before any assertion runs.
// This hook transpiles test-only component imports with the TypeScript compiler
// already used for `web-typecheck`, without adding a DOM or test framework.
// Server-side rendering needs no browser: `react-dom/server` turns the element
// into markup in plain Node.

import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";

const require = createRequire(import.meta.url);
const ts = require("typescript");
const projectRoot = new URL("../", import.meta.url);

function projectURL(relativePath) {
  return new URL(relativePath, projectRoot).href;
}

export async function resolve(specifier, context, nextResolve) {
  if (specifier.startsWith("@/")) {
    const relativePath = specifier.slice("@/".length);
    const candidates = [
      `${relativePath}.ts`,
      `${relativePath}.tsx`,
      `${relativePath}/index.ts`,
      `${relativePath}/index.tsx`,
    ];
    let lastError;
    for (const candidate of candidates) {
      try {
        const resolved = await nextResolve(projectURL(candidate), context);
        return { ...resolved, shortCircuit: true };
      } catch (error) {
        lastError = error;
      }
    }
    throw lastError;
  }
  return nextResolve(specifier, context);
}

export async function load(url, context, nextLoad) {
  if (!url.endsWith(".tsx")) {
    return nextLoad(url, context);
  }

  const source = await readFile(fileURLToPath(url), "utf8");
  const transpiled = ts.transpileModule(source, {
    compilerOptions: {
      jsx: ts.JsxEmit.ReactJSX,
      module: ts.ModuleKind.ESNext,
      target: ts.ScriptTarget.ES2022,
    },
    reportDiagnostics: true,
    fileName: fileURLToPath(url),
  });
  const failures = (transpiled.diagnostics ?? []).filter(
    (diagnostic) => diagnostic.category === ts.DiagnosticCategory.Error
  );
  if (failures.length > 0) {
    const messages = failures
      .map((diagnostic) => ts.flattenDiagnosticMessageText(diagnostic.messageText, "\n"))
      .join("\n");
    throw new Error(`Cannot transpile ${url} for tests:\n${messages}`);
  }
  return { format: "module", source: transpiled.outputText, shortCircuit: true };
}
