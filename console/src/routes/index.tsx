/* src/routes/index.tsx */

import { createFileRoute, redirect } from "@tanstack/react-router";

// A utility to generate a 16-character ID by truncating a v4 UUID.
const generateRandomId = () => {
	// Generate a full v4 UUID using the browser's built-in crypto API.
	// e.g., "123e4567-e89b-12d3-a456-426614174000"
	const fullUuid = crypto.randomUUID();

	// Remove hyphens to get a 32-character string.
	// e.g., "123e4567e89b12d3a456426614174000"
	const compactUuid = fullUuid.replace(/-/g, "");

	// Take the first 16 characters.
	return compactUuid.substring(0, 16);
};

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
