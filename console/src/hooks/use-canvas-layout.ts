/* src/hooks/use-canvas-layout.ts */

import { useState, useEffect, useCallback } from "react";
import { type UseQueryResult } from "@tanstack/react-query";
import {
	loadLayout,
	saveLayout,
	type CanvasLayout,
	type CanvasNode,
	type EntryPointNodeData,
	type RateLimitNodeData,
} from "~/lib/canvas-layout";
import { type RequestResult } from "~/api/request";

interface RateLimitConfig {
	requests_per_second: number;
}

interface UseCanvasLayoutProps {
	selectedDomain: string | null;
	rateLimitQuery: UseQueryResult<RequestResult<RateLimitConfig>>;
}

/**
 * Manages the canvas layout state, including loading from localStorage,
 * synchronizing with server data, and saving changes.
 */
export function useCanvasLayout({
	selectedDomain,
	rateLimitQuery,
}: UseCanvasLayoutProps) {
	const [layout, setLayout] = useState<CanvasLayout | null>(null);

	const generateDefaultLayout = useCallback((): CanvasLayout => {
		const nodes: CanvasNode<unknown>[] = [];
		const connections = [];

		nodes.push({
			id: "entry-point",
			type: "entry-point",
			x: 150,
			y: 200,
			inputs: [],
			outputs: [{ id: "output", label: "Output" }],
			data: {},
		} as CanvasNode<EntryPointNodeData>);

		const rateLimitRPS = rateLimitQuery.data?.data?.requests_per_second ?? 0;
		if (rateLimitRPS > 0) {
			nodes.push({
				id: "rate-limit",
				type: "rate-limit",
				x: 500,
				y: 200,
				inputs: [{ id: "input", label: "Input" }],
				outputs: [],
				data: { requests_per_second: rateLimitRPS },
			} as CanvasNode<RateLimitNodeData>);
			connections.push({
				id: "entry-to-ratelimit",
				fromNodeId: "entry-point",
				fromHandle: "output",
				toNodeId: "rate-limit",
				toHandle: "input",
			});
		}
		return { nodes, connections };
	}, [rateLimitQuery.data]);

	useEffect(() => {
		if (!selectedDomain || !rateLimitQuery.data) {
			setLayout(null); // Explicitly clear layout when not ready
			return;
		}

		const freshLayout = generateDefaultLayout();
		const savedLayout = loadLayout(selectedDomain);

		if (savedLayout) {
			const positionMap = new Map(
				savedLayout.nodes.map((node) => [node.id, { x: node.x, y: node.y }])
			);
			freshLayout.nodes.forEach((node) => {
				const savedPosition = positionMap.get(node.id);
				if (savedPosition) {
					node.x = savedPosition.x;
					node.y = savedPosition.y;
				}
			});
		}

		setLayout(freshLayout);
		saveLayout(selectedDomain, freshLayout);
	}, [selectedDomain, rateLimitQuery.data, generateDefaultLayout]);

	const handleLayoutChange = useCallback(
		(newLayout: CanvasLayout) => {
			setLayout(newLayout);
			if (selectedDomain) {
				saveLayout(selectedDomain, newLayout);
			}
		},
		[selectedDomain]
	);

	return { layout, handleLayoutChange };
}
