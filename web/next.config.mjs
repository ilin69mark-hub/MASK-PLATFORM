/** @type {import('next').NextConfig} */
const nextConfig = {
  reactStrictMode: true,
  // Dev and build get their own directory.
  //
  // They shared `.next`, which meant any `next build` — and `make check` runs
  // one — overwrote the manifest of a running dev server. Every request then
  // failed with `Cannot find module './NNN.js'`: the server bundle named a
  // chunk that the build had just deleted. Clearing the directory bought one
  // page load before the next check broke it again, so it was fixed twice and
  // returned both times.
  //
  // Two writers in one directory is a conflict, not a cache problem, and the
  // answer is two directories rather than more clearing.
  distDir: process.env.NODE_ENV === "development" ? ".next-dev" : ".next",
  // The wasm artifact is produced by `make wasm-dist` and served as a static
  // file. Nothing about it should be bundled or rewritten: the kernel is a
  // separate artifact with its own hash, and rewriting it would break the
  // cross-runtime agreement the contract check verifies.
  async headers() {
    return [
      {
        // wasm-pack's glue loads the .wasm next to the JS, and a wrong MIME type
        // makes the browser refuse to stream-compile it.
        //
        // `.wasm` only. This used to match every file under /pkg, which served the
        // glue JavaScript as `application/wasm` as well — and a browser refuses a
        // module script whose MIME type is not JavaScript. The editor could not
        // have started even with the import fixed, and the failure would have
        // looked like a missing file rather than a wrong header.
        source: "/pkg/:path*.wasm",
        headers: [
          { key: "Content-Type", value: "application/wasm" },
        ],
      },
    ];
  },
};

export default nextConfig;
