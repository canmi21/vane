/* src/api/request.ts */

import { generateToken } from "./auth";

// Defines the structure of the API response from the backend.
interface ApiResponse<T> {
	status: "success" | "error";
	data?: T;
	message?: string;
}

// Defines the unified return type for our request functions.
export interface RequestResult<T> {
	statusCode: number;
	data: T | null;
	message: string | null;
}

/**
 * A generic fetch wrapper for making authenticated API requests.
 * @param url The full URL to request.
 * @param options Standard Fetch API options.
 * @returns {Promise<RequestResult<T>>} A promise resolving to a structured result.
 */
async function makeRequest<T>(
	url: string,
	options: RequestInit = {}
): Promise<RequestResult<T>> {
	const token = await generateToken();
	if (!token) {
		return {
			statusCode: 401,
			data: null,
			message:
				"Failed to generate authentication token. Is an instance configured?",
		};
	}

	const headers = new Headers(options.headers);
	headers.set("Authorization", `Bearer ${token}`);

	try {
		const response = await fetch(url, { ...options, headers });
		const responseData: ApiResponse<T> = await response.json();

		if (responseData.status === "success") {
			return {
				statusCode: response.status,
				data: responseData.data ?? null,
				message: null,
			};
		} else {
			return {
				statusCode: response.status,
				data: null,
				message: responseData.message ?? "An unknown error occurred.",
			};
		}
	} catch (error) {
		console.error(`API request to ${url} failed:`, error);
		return {
			statusCode: 0, // 0 indicates a network or parsing error
			data: null,
			message:
				error instanceof Error ? error.message : "Network request failed.",
		};
	}
}

// Export convenient methods for different HTTP verbs.
export const http = {
	get: <T>(url: string) => makeRequest<T>(url, { method: "GET" }),
	// FIX: Replaced `any` with `Record<string, unknown>` for better type safety.
	post: <T>(url: string, body: Record<string, unknown>) =>
		makeRequest<T>(url, {
			method: "POST",
			body: JSON.stringify(body),
			headers: { "Content-Type": "application/json" },
		}),
	put: <T>(url: string, body: Record<string, unknown>) =>
		makeRequest<T>(url, {
			method: "PUT",
			body: JSON.stringify(body),
			headers: { "Content-Type": "application/json" },
		}),
	delete: <T>(url: string) => makeRequest<T>(url, { method: "DELETE" }),
	// Add other methods like put, patch as needed.
};
