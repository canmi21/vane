/* src/lib/canvas-layout.ts */

import { getInstance, putInstance } from "~/api/instance";
import { type VariableDefinition } from "~/hooks/use-plugin-data";

// --- Type Definitions ---

export interface NodeHandle {
	id: string;
	label: string;
}

// A generic CanvasNode that can hold any kind of data.
export interface CanvasNode<T = Record<string, unknown>> {
	id: string;
	type: string; // e.g., "entry-point", "ratelimit", "error-page"
	version?: string; // e.g., "v1", to track the plugin version
	x: number;
	y: number;
	inputs: NodeHandle[];
	outputs: NodeHandle[];
	data: T;
	variables?: Record<string, VariableDefinition>;
}

export interface CanvasConnection {
	id: string;
	fromNodeId: string;
	fromHandle: string;
	toNodeId: string;
	toHandle: string;
}

export interface CanvasLayout {
	nodes: CanvasNode<any>[];
	connections: CanvasConnection[];
}

// --- Specific Node Data Types (if any) ---

// The entry point node has no specific editable data.
export type EntryPointNodeData = Record<string, never>;

// --- API & LocalStorage Functions ---

const LOCAL_STORAGE_PREFIX = "vane-canvas-layout-";

export const getLayoutConfig = (instanceId: string, domain: string) =>
	getInstance<CanvasLayout>(instanceId, `/v1/layout/${domain}`);

export const updateLayoutConfig = (
	instanceId: string,
	domain: string,
	layout: CanvasLayout
) => putInstance(instanceId, `/v1/layout/${domain}`, layout);

export function getLayoutFromLocalStorage(domain: string): CanvasLayout | null {
	try {
		const stored = localStorage.getItem(LOCAL_STORAGE_PREFIX + domain);
		return stored ? JSON.parse(stored) : null;
	} catch (error) {
		console.error("Failed to parse layout from LocalStorage:", error);
		return null;
	}
}

export function saveLayoutToLocalStorage(domain: string, layout: CanvasLayout) {
	try {
		localStorage.setItem(LOCAL_STORAGE_PREFIX + domain, JSON.stringify(layout));
	} catch (error) {
		console.error("Failed to save layout to LocalStorage:", error);
	}
}
