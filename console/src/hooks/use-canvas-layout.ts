/* src/hooks/use-canvas-layout.ts */

import { useState, useEffect, useCallback } from "react";
import {
	loadLayout,
	saveLayout,
	type CanvasLayout,
	type CanvasNode,
	type EntryPointNodeData,
} from "~/lib/canvas-layout";
import { nanoid } from "nanoid";
import { type Plugin } from "./use-plugin-data"; // Import the new Plugin type

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

	// --- FINAL FIX: Generalize addNode to work with any plugin definition ---
	const addNode = useCallback(
		(plugin: Plugin) => {
			if (!layout) return;

			// Generate default data based on the plugin's input_params definition.
			const defaultData: Record<string, unknown> = {};
			for (const key in plugin.input_params) {
				const param = plugin.input_params[key];
				if (param.type === "number") {
					// A sensible default for rate limiting.
					defaultData[key] = key === "requests_per_second" ? 100 : 0;
				} else {
					defaultData[key] = "";
				}
			}

			const newNode: CanvasNode = {
				id: nanoid(8),
				type: plugin.name, // The node type is now the plugin name.
				x: 350,
				y: 350,
				// All plugins have a single input handle by convention.
				inputs: [{ id: "input", label: "Input" }],
				// Outputs are dynamically generated from the plugin's 'tree'.
				outputs: plugin.output_results.tree.map((handle) => ({
					id: handle,
					label: handle.charAt(0).toUpperCase() + handle.slice(1),
				})),
				data: defaultData,
			};

			const newLayout = { ...layout, nodes: [...layout.nodes, newNode] };
			handleLayoutChange(newLayout);
		},
		[layout, handleLayoutChange]
	);

	return { layout, handleLayoutChange, addNode };
}
