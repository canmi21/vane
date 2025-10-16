import type { Config } from "tailwindcss"

export default {
	darkMode: "media",
	content: ["./src/**/*.{astro,html,js,ts,jsx,tsx}"],
	theme: {
		extend: {
			colors: {
				vane: {
					light: "#f9fafb",
					dark: "#0f0f10"
				}
			}
		}
	},
	plugins: []
} satisfies Config
