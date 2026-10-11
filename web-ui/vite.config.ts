import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwind from "@tailwindcss/vite";
import { viteSingleFile } from "vite-plugin-singlefile";
import path from "node:path";

// The studio ships inside the Rust binary as one self-contained HTML file, so a
// plain `cargo build` never needs Node or Bun. `bun run build` refreshes it.
export default defineConfig({
  plugins: [react(), tailwind(), viteSingleFile()],
  resolve: { alias: { "@": path.resolve(__dirname, "src") } },
  server: {
    proxy: { "/api": "http://127.0.0.1:7878", "/fonts": "http://127.0.0.1:7878" },
  },
  build: { target: "es2022", chunkSizeWarningLimit: 1500 },
});
