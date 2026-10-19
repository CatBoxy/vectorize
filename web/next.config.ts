import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  // The tracer runs in a Web Worker as WebAssembly served from public/wasm
  // (copied there by `npm run build:wasm`). Nothing is uploaded, so no
  // image optimisation, no API routes, no telemetry of our own.
  reactStrictMode: true,
  async headers() {
    return [
      {
        source: "/wasm/:path*",
        headers: [{ key: "Cache-Control", value: "public, max-age=31536000, immutable" }],
      },
    ];
  },
};

export default nextConfig;
