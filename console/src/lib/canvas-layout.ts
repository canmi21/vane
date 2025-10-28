/* src/lib/canvas-layout.ts */

// --- Type Definitions ---

/**
 * Represents a single input or output point on a node.
 */
export interface NodeHandle {
	id: string; // A unique identifier, e.g., "in_main", "out_success"
	label: string; // A user-friendly display name, e.g., "Input", "Success"
}

/**
 * Represents a generic node on the canvas.
 * Uses a generic type `T` for its specific configuration data.
 */
export interface CanvasNode<T = unknown> {
	id: string;
	type: string; // e.g., "entry-point", "rate-limit"
	x: number;
	y: number;
	inputs: NodeHandle[];
	outputs: NodeHandle[];
	data: T; // Node-specific data, e.g., { requests_per_second: 100 }
}

/**
 * Specific data shapes for our existing node types.
 */
export type EntryPointNodeData = Record<string, never>; // No special data yet
export interface RateLimitNodeData {
	requests_per_second: number;
}

/**
 * Represents a connection between two node handles.
 */
export interface CanvasConnection {
	id: string;
	fromNodeId: string;
	fromHandle: string; // Corresponds to a NodeHandle `id` on the source node
	toNodeId: string;
	toHandle: string; // Corresponds to a NodeHandle `id` on the target node
}

/**
 * The complete layout definition for a canvas instance.
 */
export interface CanvasLayout {
	nodes: CanvasNode<unknown>[]; // An array of nodes with potentially different data types
	connections: CanvasConnection[];
}

// --- LocalStorage Logic ---
function getStorageKey(domain: string): string {
	return `@vane/canvas-layout/${domain}`;
}

export function loadLayout(domain: string): CanvasLayout | null {
	const data = localStorage.getItem(getStorageKey(domain));
	if (!data) return null;
	try {
		// A simple validation to ensure the loaded data has the expected shape.
		const parsed = JSON.parse(data);
		if (parsed.nodes && parsed.connections) {
			return parsed as CanvasLayout;
		}
		return null;
	} catch {
		return null;
	}
}

export function saveLayout(domain: string, layout: CanvasLayout): void {
	localStorage.setItem(getStorageKey(domain), JSON.stringify(layout));
}
