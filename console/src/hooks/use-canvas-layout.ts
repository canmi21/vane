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
import { type Plugin } from "./use-plugin-data";

interface UseCanvasLayoutProps {
	selectedDomain: string | null;
}

/**
 * Manages the canvas layout state, persistence, and node modifications.
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

	const addNode = useCallback(
		(plugin: Plugin) => {
			if (!layout) return;

			const defaultData: Record<string, unknown> = {};
			for (const key in plugin.input_params) {
				const param = plugin.input_params[key];
				if (param.type === "number") {
					defaultData[key] = key.includes("requests") ? 100 : 0;
				} else if (param.type === "boolean") {
					defaultData[key] = false;
				} else {
					defaultData[key] = "";
				}
			}

			const newNode: CanvasNode = {
				id: nanoid(8),
				type: plugin.name,
				x: 350,
				y: 350,
				inputs: [{ id: "input", label: "Input" }],
				outputs: plugin.output_results.tree.map((handle) => ({
					id: handle,
					label: handle.charAt(0).toUpperCase() + handle.slice(1),
				})),
				data: defaultData,
			};

			handleLayoutChange({ ...layout, nodes: [...layout.nodes, newNode] });
		},
		[layout, handleLayoutChange]
	);

	// --- FINAL FIX: Add a function to update a specific node's data and persist it ---
	const updateNodeData = useCallback(
		(nodeId: string, newData: Record<string, unknown>) => {
			if (!layout) return;

			const newNodes = layout.nodes.map((n) =>
				n.id === nodeId ? { ...n, data: newData } : n
			);
			handleLayoutChange({ ...layout, nodes: newNodes });
		},
		[layout, handleLayoutChange]
	);

	return { layout, handleLayoutChange, addNode, updateNodeData };
}
