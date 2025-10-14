/* tailwind.config.js */

/** @type {import('tailwindcss').Config} */
export default {
	darkMode: "class",
	content: ["./index.html", "./src/**/*.{js,ts,jsx,tsx,mdx}"],
	theme: {
		extend: {
			keyframes: {
				twinkle: {
					"0%": {
						opacity: "0.2",
						transform: "scale(0.8)",
					},
					"100%": {
						opacity: "0.8",
						transform: "scale(1.2)",
					},
				},
			},
			animation: {
				twinkle: "twinkle infinite ease-in-out alternate",
				"spin-slow": "spin 15s linear infinite",
			},
		},
	},
	plugins: [],
};
