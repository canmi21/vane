/* src/hooks/use-connection-points.ts */

import { useCallback } from "react";
import { type CanvasLayout } from "~/lib/canvas-layout";

/**
 * A hook that provides a memoized function to calculate the absolute
 * coordinates of any node handle on the canvas.
 */
export function useConnectionPoints(layout: CanvasLayout) {
	const getConnectionPoints = useCallback(
		(nodeId: string, handleId: string): { x: number; y: number } => {
			const node = layout.nodes.find((n) => n.id === nodeId);
			if (!node) return { x: 0, y: 0 };

			const nodeWidth = 256;
			const headerHeight = 41; // This must match the value in CanvasNodeCard.tsx

			if (node.type === "entry-point") {
				const totalHeight = 83; // Height of the entry point card.
				return { x: node.x + nodeWidth, y: node.y + totalHeight / 2 };
			}

			// --- FINAL FIX: Generic logic for all plugin-based nodes ---
			// This logic now mirrors the rendering logic in CanvasNodeCard.tsx.
			const isInput = node.inputs.some((h) => h.id === handleId);

			if (isInput) {
				const bodyTopAbsoluteY = node.y + headerHeight;
				// The input handle is always centered in the first "unit" of the body.
				const handleOffsetY = headerHeight / 2;
				return { x: node.x, y: bodyTopAbsoluteY + handleOffsetY };
			} else {
				const outputIndex = node.outputs.findIndex((h) => h.id === handleId);
				if (outputIndex === -1) return { x: node.x, y: node.y }; // Fallback

				const bodyHeight =
					headerHeight * (node.outputs.length > 0 ? node.outputs.length : 1);

				// This calculation must be identical to the one in CanvasNodeCard.
				const positionPercent =
					node.outputs.length <= 1
						? 50
						: (100 / (node.outputs.length + 1)) * (outputIndex + 1);

				const outputY =
					node.y + headerHeight + bodyHeight * (positionPercent / 100);
				return { x: node.x + nodeWidth, y: outputY };
			}
		},
		[layout.nodes]
	);

	return { getConnectionPoints };
}
