import { readFileSync } from "node:fs";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const brand = JSON.parse(readFileSync(new URL("./brand.json", import.meta.url), "utf8"));

// Tauri v2: the frontend talks to Rust via IPC (@tauri-apps/api), so no HTTP proxy.
export default defineConfig({
  plugins: [
    react(),
    {
      // index.html uses %BRAND_NAME% / %BRAND_TAGLINE% so the boot screen matches brand.json.
      name: "brand-html",
      transformIndexHtml: (html) =>
        html.replaceAll("%BRAND_NAME%", brand.name).replaceAll("%BRAND_TAGLINE%", brand.tagline),
    },
  ],
  clearScreen: false,
  // The bundle loads from disk inside the app, so web chunk-size advice doesn't apply.
  build: { chunkSizeWarningLimit: 2000 },
  base: './',
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});
