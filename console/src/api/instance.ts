/* src/api/instance.ts */

import { http, type RequestResult } from "./request";

// LocalStorage keys
const LS_DEFAULT_INSTANCE_KEY = "@vane/default-instance";
const LS_INSTANCES_KEY = "@vane/instance";

/**
 * Retrieves the base URL for a given instance ID.
 * @param instanceId The ID of the instance to look up.
 * @returns {string | null} The base URL or null if not found.
 */
function getBaseUrl(instanceId: string): string | null {
	const allInstancesRaw = localStorage.getItem(LS_INSTANCES_KEY);
	if (!allInstancesRaw) return null;

	try {
		const allInstances = JSON.parse(allInstancesRaw);
		return allInstances[instanceId]?.baseUrl || null;
	} catch {
		return null;
	}
}

/**
 * Returns the ID of the currently active instance.
 * @returns {string | null} The active instance ID.
 */
export function getActiveInstanceId(): string | null {
	return localStorage.getItem(LS_DEFAULT_INSTANCE_KEY);
}

/**
 * Makes an authenticated GET request to a specific path of a given instance.
 * @param instanceId The ID of the target instance.
 * @param path The API path to request (e.g., "/v1/instance").
 * @returns {Promise<RequestResult<T>>} A promise with the structured API result.
 */
export async function getInstance<T>(
	instanceId: string,
	path: string
): Promise<RequestResult<T>> {
	const baseUrl = getBaseUrl(instanceId);
	if (!baseUrl) {
		return {
			statusCode: 404,
			data: null,
			message: `Configuration for instance "${instanceId}" not found.`,
		};
	}
	return http.get<T>(`${baseUrl}${path}`);
}
