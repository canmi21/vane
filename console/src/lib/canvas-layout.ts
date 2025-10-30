/* src/lib/canvas-layout.ts */

export interface NodeHandle {
	id: string;
	label: string;
}
export interface CanvasNode<T = unknown> {
	id: string;
	type: string;
	x: number;
	y: number;
	inputs: NodeHandle[];
	outputs: NodeHandle[];
	data: T;
}
export type EntryPointNodeData = Record<string, never>;
export interface RateLimitNodeData {
	requests_per_second: number;
}
export interface CanvasConnection {
	id: string;
	fromNodeId: string;
	fromHandle: string;
	toNodeId: string;
	toHandle: string;
}
export interface CanvasLayout {
	nodes: CanvasNode<unknown>[];
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
