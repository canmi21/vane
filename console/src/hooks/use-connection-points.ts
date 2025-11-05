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

			// --- FINAL FIX: Refactor into a clear if-else if-else structure to handle each node type correctly. ---

			// 1. Handle the 'entry-point' node.
			if (node.type === "entry-point") {
				const totalHeight = 83;
				return { x: node.x + nodeWidth, y: node.y + totalHeight / 2 };
			}
			// 2. Handle all terminal built-in nodes.
			else if (node.type === "error-page" || node.type === "return-response") {
				const isInput = node.inputs.some((h) => h.id === handleId);
				if (isInput) {
					// These nodes have no outputs, so vertically center the input handle.
					const inputHandleY = headerHeight / 2;
					return { x: node.x, y: node.y + headerHeight + inputHandleY };
				}
				return { x: 0, y: 0 }; // No outputs
			}
			// 3. Handle all other nodes (assumed to be plugins).
			else {
				const plugin = plugins.find((p) => p.name === node.type);

				const heightFromOutputs =
					headerHeight * (node.outputs.length > 0 ? node.outputs.length : 1);

				const inputParamCount = plugin
					? Object.keys(plugin.input_params).length
					: 0;

				const ROW_HEIGHT = 52;
				const GAP_HEIGHT = 8;
				const PADDING_VERTICAL = 24;

				const heightFromInputs =
					inputParamCount > 0
						? inputParamCount * ROW_HEIGHT +
							(inputParamCount - 1) * GAP_HEIGHT +
							PADDING_VERTICAL
						: PADDING_VERTICAL;

				const bodyHeight = Math.max(heightFromOutputs, heightFromInputs);

				const isInput = node.inputs.some((h) => h.id === handleId);

				if (isInput) {
					const firstOutputPositionPercent =
						node.outputs.length <= 1 ? 50 : 100 / (node.outputs.length + 1);
					const inputHandleY =
						node.outputs.length > 0
							? bodyHeight * (firstOutputPositionPercent / 100)
							: headerHeight / 2; // Fallback if no outputs.
					return { x: node.x, y: node.y + headerHeight + inputHandleY };
				} else {
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
			}
		},
		[layout.nodes, plugins]
	);

	return { getConnectionPoints };
}
