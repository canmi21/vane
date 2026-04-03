import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

export default defineConfig({
  plugins: [svelte()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
  server: {
    proxy: {
      "/_bridge": {
        target: "http://127.0.0.1:3333",
        changeOrigin: true,
      },
    },
  },
});
