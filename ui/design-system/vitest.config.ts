import { defineConfig } from "vitest/config";

// Component tests render in jsdom; the code under test is plain TSX, which
// Vitest's own transform handles.
export default defineConfig({
  test: {
    include: ["src/**/*.test.tsx", "src/**/*.test.ts"],
    environment: "jsdom",
  },
  esbuild: { jsx: "automatic" },
});
