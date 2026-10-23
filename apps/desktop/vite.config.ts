import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import { resolveBuildTargetDir } from "../../scripts/build-target.mjs";

const cargoTargetDir = resolveBuildTargetDir(process.env.CARGO_TARGET_DIR);
const repositoryRoot = fileURLToPath(new URL("../..", import.meta.url));

export default defineConfig({
  cacheDir: resolve(cargoTargetDir, "vite-cache", "desktop"),
  build: {
    outDir: resolve(cargoTargetDir, "desktop-dist"),
    emptyOutDir: true,
  },
  resolve: {
    alias: [
      { find: "@", replacement: resolve(repositoryRoot, "src") },
      {
        find: "next/image",
        replacement: resolve(repositoryRoot, "apps/desktop/src/next-image.tsx"),
      },
    ],
  },
  define: {
    "process.env.NEXT_PUBLIC_BUGLOGIN_APP_TARGET": JSON.stringify("desktop"),
    "process.env.NEXT_PUBLIC_BUGLOGIN_WORKSPACE_SCOPES": JSON.stringify("[]"),
  },
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
    watch: {
      ignored: [
        "**/src-tauri/**",
        "**/*.exe",
        "**/target/**",
        "**/binaries/**",
      ],
    },
  },
  preview: { host: "127.0.0.1", port: 1420, strictPort: true },
});
