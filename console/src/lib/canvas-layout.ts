/* src/lib/canvas-layout.ts */

import { getInstance, putInstance } from "~/api/instance";
import { type RequestResult } from "~/api/request";

// --- Type Definitions ---
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

export interface ErrorPageNodeData {
	status_code: number;
	status_description: string;
	reason: string;
	request_id: string;
	timestamp: string;
	version: string;
	request_ip: string;
	visitor_tip: string;
	admin_guide: string;
}

// --- FINAL FIX: Add and export the new interface for the Return Response node. ---
export interface ReturnResponseNodeData {
	status_code: number;
	header: string;
	body: string;
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

// --- API Functions for Backend Synchronization ---

/**
 * Fetches the layout for a domain from the backend.
 */
export const getLayoutConfig = (instanceId: string, domain: string) =>
	getInstance<CanvasLayout>(
		instanceId,
		`/v1/layout/${encodeURIComponent(domain)}`
	);

/**
 * Saves the layout for a domain to the backend.
 */
export const updateLayoutConfig = (
	instanceId: string,
	domain: string,
	layout: CanvasLayout
): Promise<RequestResult<CanvasLayout>> => {
	const body = layout as unknown as Record<string, unknown>;
	return putInstance<CanvasLayout>(
		instanceId,
		`/v1/layout/${encodeURIComponent(domain)}`,
		body
	);
};

// --- LocalStorage functions for clarity ---

function getStorageKey(domain: string): string {
	return `@vane/canvas-layout/${domain}`;
}

/**
 * Loads the layout from the browser's LocalStorage.
 */
export function loadLayoutFromLocalStorage(
	domain: string
): CanvasLayout | null {
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

/**
 * Saves the layout to the browser's LocalStorage.
 */
export function saveLayoutToLocalStorage(
	domain: string,
	layout: CanvasLayout
): void {
	localStorage.setItem(getStorageKey(domain), JSON.stringify(layout));
}
