/** @type {import('next').NextConfig} */
const nextConfig = {
  reactStrictMode: true,
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
