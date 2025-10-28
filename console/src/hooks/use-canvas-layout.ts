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

	// Generates the initial, minimal layout with only the entry point.
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

	// On domain change, load from localStorage or create a default layout.
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

	// Function to add new nodes to the canvas.
	const addNode = (type: "rate-limit") => {
		if (!layout) return;

		// This can be expanded with a switch statement for more node types.
		if (type === "rate-limit") {
			const newNode: CanvasNode<RateLimitNodeData> = {
				id: nanoid(8), // Use a shorter, unique ID
				type: "rate-limit",
				x: 350, // A sensible default position
				y: 350,
				inputs: [{ id: "input", label: "Input" }],
				outputs: [
					{ id: "accept", label: "Accept" },
					{ id: "drop", label: "Drop" },
				],
				data: { requests_per_second: 100 }, // Default data
			};

			const newLayout = { ...layout, nodes: [...layout.nodes, newNode] };
			handleLayoutChange(newLayout);
		}
	};

	return { layout, handleLayoutChange, addNode };
}
