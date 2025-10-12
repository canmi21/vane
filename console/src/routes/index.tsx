/* src/routes/index.tsx */

import { createFileRoute, redirect } from "@tanstack/react-router";

// A simple utility to generate a random string.
const generateRandomId = () => Math.random().toString(36).substring(2, 7);

const LOCAL_STORAGE_KEY = "@vane/default-instance";

export const Route = createFileRoute("/")({
	// This function runs before the component loads.
	beforeLoad: () => {
		// Try to get the default instance from localStorage.
		let instance = localStorage.getItem(LOCAL_STORAGE_KEY);

		if (!instance) {
			// If it doesn't exist, generate a new one.
			instance = generateRandomId();
			// Store it for future visits.
			localStorage.setItem(LOCAL_STORAGE_KEY, instance);
		}

		// Redirect to the instance-specific homepage.
		throw redirect({
			to: "/$instance",
			params: { instance },
		});
	},
	// This component will never be rendered due to the redirect.
	component: () => null,
});
