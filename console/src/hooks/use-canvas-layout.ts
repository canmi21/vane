/* src/hooks/use-canvas-layout.ts */

import { useState, useEffect, useCallback } from "react";
import {
	loadLayout,
	saveLayout,
	type CanvasLayout,
	type CanvasNode,
	type EntryPointNodeData,
	type RateLimitNodeData,
} from "~/lib/canvas-layout";
import { nanoid } from "nanoid";

interface UseCanvasLayoutProps {
	selectedDomain: string | null;
}

/**
 * Manages the canvas layout state. It is now the single source of truth,
 * driven by user actions and persisted in localStorage.
 */
export function useCanvasLayout({ selectedDomain }: UseCanvasLayoutProps) {
	const [layout, setLayout] = useState<CanvasLayout | null>(null);

	const generateDefaultLayout = useCallback((): CanvasLayout => {
		const entryPointNode: CanvasNode<EntryPointNodeData> = {
			id: "entry-point",
			type: "entry-point",
			x: 150,
			y: 200,
			inputs: [],
			outputs: [{ id: "output", label: "Output" }],
			data: {},
		};
		return { nodes: [entryPointNode], connections: [] };
	}, []);

	useEffect(() => {
		if (!selectedDomain) {
			setLayout(null);
			return;
		}

		const savedLayout = loadLayout(selectedDomain);
		if (savedLayout) {
			setLayout(savedLayout);
		} else {
			const newLayout = generateDefaultLayout();
			setLayout(newLayout);
			saveLayout(selectedDomain, newLayout);
		}
	}, [selectedDomain, generateDefaultLayout]);

	const handleLayoutChange = useCallback(
		(newLayout: CanvasLayout) => {
			setLayout(newLayout);
			if (selectedDomain) {
				saveLayout(selectedDomain, newLayout);
			}
		},
		[selectedDomain]
	);

	const addNode = (type: "rate-limit") => {
		if (!layout) return;

		if (type === "rate-limit") {
			const newNode: CanvasNode<RateLimitNodeData> = {
				id: nanoid(8),
				type: "rate-limit",
				x: 350,
				y: 350,
				inputs: [{ id: "input", label: "Input" }],
				outputs: [
					{ id: "accept", label: "Accept" },
					{ id: "drop", label: "Drop" },
				],
				data: { requests_per_second: 100 },
			};

			const newLayout = { ...layout, nodes: [...layout.nodes, newNode] };
			handleLayoutChange(newLayout);
		}
	};

	return { layout, handleLayoutChange, addNode };
}
