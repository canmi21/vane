/* src/lib/canvas-layout.ts */

// --- Type Definitions ---
export interface CanvasNode {
	id: string;
	type: "entry-point" | "rate-limit"; // Add more types later
	x: number;
	y: number;
}
export interface CanvasConnection {
	id: string;
	fromNodeId: string;
	fromHandle: string; // e.g., 'output'
	toNodeId: string;
	toHandle: string; // e.g., 'input'
}
export interface CanvasLayout {
	nodes: CanvasNode[];
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
		return JSON.parse(data) as CanvasLayout;
	} catch {
		return null;
	}
}
export function saveLayout(domain: string, layout: CanvasLayout): void {
	localStorage.setItem(getStorageKey(domain), JSON.stringify(layout));
}
