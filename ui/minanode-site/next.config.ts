import path from "node:path";
import type { NextConfig } from "next";

// The npm workspace root (ui/), where node_modules lives.
const workspaceRoot = path.resolve(process.cwd(), "..");

const nextConfig: NextConfig = {
  // Static export: plain files in out/, no server at runtime.
  output: "export",
  // /design-system/ becomes design-system/index.html, which any static host
  // serves without rewrites.
  trailingSlash: true,
  images: { unoptimized: true },
  transpilePackages: ["@minanode/design-system"],
  reactStrictMode: true,
  turbopack: { root: workspaceRoot },
  outputFileTracingRoot: workspaceRoot,
};

export default nextConfig;
