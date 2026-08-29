import { defineConfig } from "vite";
import { resolve } from "node:path";
import { copyFileSync, rmSync } from "node:fs";

const root = import.meta.dirname;
const buildDirectory = resolve(root, "../.work/web-build");

export default defineConfig({
  plugins: [{
    name: "opencoding-checked-in-web-bundle",
    closeBundle() {
      copyFileSync(resolve(buildDirectory, "app.js"), resolve(root, "app.js"));
      rmSync(buildDirectory, { recursive: true, force: true });
    }
  }],
  build: {
    outDir: buildDirectory,
    emptyOutDir: true,
    minify: false,
    sourcemap: false,
    lib: {
      entry: resolve(import.meta.dirname, "src/main.ts"),
      formats: ["es"],
      fileName: () => "app.js"
    }
  }
});
