import { defineConfig, type Plugin } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

// pdf.js loads fonts, character maps, colour profiles and image decoders at
// run time; serve them from /pdfjs/ (bundled with the app, nothing online).
const PDFJS = "node_modules/pdfjs-dist";
const PDFJS_DIRS = ["cmaps", "standard_fonts", "wasm", "iccs"];
function pdfjsAssets(): Plugin {
  return {
    name: "pdfjs-assets",
    configureServer(server) {
      server.middlewares.use("/pdfjs/", (req, res, next) => {
        const rel = decodeURIComponent((req.url ?? "").split("?")[0]).replace(/^\/+/, "");
        if (!PDFJS_DIRS.includes(rel.split("/")[0]) || rel.includes("..")) return next();
        try {
          const body = readFileSync(join(PDFJS, rel));
          if (rel.endsWith(".wasm")) res.setHeader("Content-Type", "application/wasm");
          res.end(body);
        } catch {
          next();
        }
      });
    },
    generateBundle() {
      for (const dir of PDFJS_DIRS) {
        for (const name of readdirSync(join(PDFJS, dir))) {
          const path = join(PDFJS, dir, name);
          if (statSync(path).isFile()) this.emitFile({ type: "asset", fileName: `pdfjs/${dir}/${name}`, source: readFileSync(path) });
        }
      }
    },
  };
}

// Tauri expects a fixed port and must not have the terminal cleared.
export default defineConfig({
  plugins: [svelte(), pdfjsAssets()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ["**/src-tauri/**"] } },
  build: { target: "es2022", sourcemap: false },
});
