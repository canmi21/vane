/* vite.config.ts */

import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { tanstackRouter } from "@tanstack/router-vite-plugin";
import path from "path";
import tailwindcss from "@tailwindcss/vite";
import svgr from "vite-plugin-svgr";
import wasm from "vite-plugin-wasm";
import { execSync } from "child_process";

const gitCommitHash = execSync("git rev-parse --short HEAD").toString().trim();

// https://vite.dev/config/
export default defineConfig({
	plugins: [
		tanstackRouter({
			target: "react",
			autoCodeSplitting: true,
		}),
		react(),
		wasm(),
		tailwindcss(),
		svgr(),
	],
	resolve: {
		alias: {
			"~": path.resolve(__dirname, "./src"),
		},
	},
	define: {
    __GIT_HASH__: JSON.stringify(gitCommitHash),
  },
});
