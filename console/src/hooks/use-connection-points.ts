/* src/hooks/use-connection-points.ts */

import { useCallback } from "react";
import { type CanvasLayout } from "~/lib/canvas-layout";
import { type Plugin } from "./use-plugin-data"; // Import Plugin type

/**
 * A hook that provides a memoized function to calculate the absolute
 * coordinates of any node handle on the canvas.
 */
export function useConnectionPoints(layout: CanvasLayout, plugins: Plugin[]) {
	const getConnectionPoints = useCallback(
		(nodeId: string, handleId: string): { x: number; y: number } => {
			const node = layout.nodes.find((n) => n.id === nodeId);
			if (!node) return { x: 0, y: 0 };

			const nodeWidth = 256;
			const headerHeight = 41;

			if (node.type === "entry-point") {
				const totalHeight = 83;
				return { x: node.x + nodeWidth, y: node.y + totalHeight / 2 };
			}

			// --- FINAL FIX: Generic logic that mirrors CanvasNodeCard's rendering ---
			const plugin = plugins.find((p) => p.name === node.type);

			// Calculate bodyHeight dynamically, just like in the rendering component.
			const heightFromOutputs =
				headerHeight * (node.outputs.length > 0 ? node.outputs.length : 1);
			const inputParamCount = plugin
				? Object.keys(plugin.input_params).length
				: 0;
			const ESTIMATED_INPUT_ROW_HEIGHT = 60;
			const PADDING = 24;
			const heightFromInputs =
				inputParamCount * ESTIMATED_INPUT_ROW_HEIGHT + PADDING;
			const bodyHeight = Math.max(heightFromOutputs, heightFromInputs);

			const isInput = node.inputs.some((h) => h.id === handleId);

			if (isInput) {
				// Align the input handle with the first output handle.
				const firstOutputPositionPercent =
					node.outputs.length <= 1 ? 50 : 100 / (node.outputs.length + 1);
				const inputHandleY =
					node.outputs.length > 0
						? bodyHeight * (firstOutputPositionPercent / 100)
						: headerHeight / 2; // Fallback if no outputs.
				return { x: node.x, y: node.y + headerHeight + inputHandleY };
			} else {
				// Evenly space the output handles along the new dynamic bodyHeight.
				const outputIndex = node.outputs.findIndex((h) => h.id === handleId);
				if (outputIndex === -1) return { x: node.x, y: node.y }; // Fallback

				const positionPercent =
					node.outputs.length <= 1
						? 50
						: (100 / (node.outputs.length + 1)) * (outputIndex + 1);
				const outputY =
					node.y + headerHeight + bodyHeight * (positionPercent / 100);
				return { x: node.x + nodeWidth, y: outputY };
			}
		},
		[layout.nodes, plugins]
	);

	return { getConnectionPoints };
}
