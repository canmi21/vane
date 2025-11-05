/* src/hooks/use-connection-points.ts */

import { useCallback } from "react";
import { type CanvasLayout } from "~/lib/canvas-layout";
import { type Plugin } from "./use-plugin-data";

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

			// 1. Handle the 'entry-point' node.
			if (node.type === "entry-point") {
				const totalHeight = 83;
				return { x: node.x + nodeWidth, y: node.y + totalHeight / 2 };
			}

			// 2. Handle all other nodes (plugins, including terminal ones).
			const plugin = plugins.find(
				(p) => p.name === node.type && p.version === node.version
			);

			const isTerminal = node.outputs.length === 0;

			const heightFromOutputs = headerHeight * node.outputs.length;

			const inputParamCount = plugin
				? Object.keys(plugin.input_params).length
				: 0;

			// This height calculation must stay in sync with CanvasNodeCard.tsx
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
				// --- FINAL FIX: This logic now precisely mirrors the visual rendering in CanvasNodeCard.tsx ---
				let inputHandleRelativeY: number;

				if (isTerminal) {
					// For TERMINAL nodes, the handle has a fixed position relative to the header height,
					// creating that "top-left" alignment you described.
					inputHandleRelativeY = headerHeight / 2;
				} else {
					// For MIDDLEWARE nodes, the handle's position is determined by the
					// position of the first output handle, ensuring they align perfectly.
					const firstOutputPositionPercent = 100 / (node.outputs.length + 1);
					inputHandleRelativeY =
						bodyHeight * (firstOutputPositionPercent / 100);
				}

				// The absolute position is the node's top + header height + the handle's relative position within the body.
				return { x: node.x, y: node.y + headerHeight + inputHandleRelativeY };
			} else {
				// This logic for output handles remains correct.
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
