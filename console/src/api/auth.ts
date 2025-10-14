/* src/api/auth.ts */

// FIX: Changed import to let the bundler handle WASM loading.
import init, { wasm_generate_combined_token } from "~/../pkg/totp/totp.js";

// Define the structure of data in localStorage
interface InstanceDetails {
	baseUrl: string;
	os: string;
	seeds: string[];
}

// LocalStorage keys
const LS_DEFAULT_INSTANCE_KEY = "@vane/default-instance";
const LS_INSTANCES_KEY = "@vane/instance";

// Initialize the WASM module once.
// FIX: Calling init() without parameters is the modern way that avoids the warning.
// The vite-plugin-wasm will handle providing the correct path.
const wasmModule = init();

/**
 * Retrieves the seeds for the currently active instance.
 * @returns {string[] | null} An array of 6 seed strings or null if not found.
 */
function getActiveInstanceSeeds(): string[] | null {
	const defaultInstanceId = localStorage.getItem(LS_DEFAULT_INSTANCE_KEY);
	if (!defaultInstanceId) {
		console.error("No default instance is selected.");
		return null;
	}

	const allInstancesRaw = localStorage.getItem(LS_INSTANCES_KEY);
	if (!allInstancesRaw) {
		console.error("Instances data not found in localStorage.");
		return null;
	}

	try {
		const allInstances: Record<string, InstanceDetails> =
			JSON.parse(allInstancesRaw);
		const activeInstance = allInstances[defaultInstanceId];

		if (
			!activeInstance ||
			!Array.isArray(activeInstance.seeds) ||
			activeInstance.seeds.length < 6
		) {
			console.error(
				`Seeds for instance "${defaultInstanceId}" are missing or invalid.`
			);
			return null;
		}

		return activeInstance.seeds;
	} catch (error) {
		console.error("Failed to parse instances data from localStorage:", error);
		return null;
	}
}

/**
 * Generates a TOTP token for the currently active instance.
 * @returns {Promise<string | null>} A promise that resolves to the token string or null on failure.
 */
export async function generateToken(): Promise<string | null> {
	await wasmModule; // Ensure WASM is loaded

	const seeds = getActiveInstanceSeeds();
	if (!seeds) {
		return null;
	}

	// Calculate the current Unix time in seconds.
	const currentTime = BigInt(Math.floor(Date.now() / 1000));
	const windowSize = BigInt(30);

	// Generate the token using the WASM function.
	const token = wasm_generate_combined_token(seeds, currentTime, windowSize);

	return token;
}
