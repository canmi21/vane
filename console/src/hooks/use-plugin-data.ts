/* src/hooks/use-plugin-data.ts */

import { useQuery } from "@tanstack/react-query";
import { getInstance } from "~/api/instance";

// --- Type Definitions (matching the backend) ---

export interface PluginInterface {
	type: string;
}

export interface ParamDefinition {
	type: string;
}

export interface VariableDefinition {
	type: string;
}

export interface Author {
	name: string;
	url: string;
}

export interface OutputResults {
	tree: string[];
	variables: Record<string, VariableDefinition>;
	return?: boolean;
}

export interface Plugin {
	name: string;
	version: string;
	interface: PluginInterface;
	description: string;
	authors: Author[];
	input_params: Record<string, ParamDefinition>;
	output_results: OutputResults;
}

interface AllPluginsResponse {
	internal: Plugin[];
	external: Plugin[];
}

// --- API Function ---

const listPlugins = (instanceId: string) =>
	getInstance<AllPluginsResponse>(instanceId, "/v1/plugins");

// --- Hook ---

/**
 * A hook to fetch and provide the list of all available plugins.
 */
export function usePluginData(instanceId: string) {
	return useQuery({
		queryKey: ["instance", instanceId, "plugins"],
		queryFn: () => listPlugins(instanceId),
		// Plugins are not expected to change often during a session.
		staleTime: Infinity,
	});
}
