/* src/hooks/use-connection-points.ts */

import { useCallback } from "react";
import {
	type CanvasLayout,
	type CanvasNode,
	type RateLimitNodeData,
} from "~/lib/canvas-layout";

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
			const headerHeight = 41;

			if (node.type === "entry-point") {
				const totalHeight = 83;
				return { x: node.x + nodeWidth, y: node.y + totalHeight / 2 };
			}

			if (node.type === "rate-limit") {
				const typedNode = node as CanvasNode<RateLimitNodeData>;
				const isInput = typedNode.inputs.some((h) => h.id === handleId);

				if (isInput) {
					const bodyTopAbsoluteY = typedNode.y + headerHeight;
					const handleOffsetY = headerHeight / 2;
					return { x: typedNode.x, y: bodyTopAbsoluteY + handleOffsetY };
				} else {
					const bodyHeight =
						headerHeight *
						(typedNode.outputs.length > 0 ? typedNode.outputs.length : 1);
					const outputIndex = typedNode.outputs.findIndex(
						(h) => h.id === handleId
					);
					if (outputIndex === -1) return { x: typedNode.x, y: typedNode.y };

					const positionPercent =
						typedNode.outputs.length <= 1
							? 50
							: (100 / (typedNode.outputs.length + 1)) * (outputIndex + 1);
					const outputY =
						typedNode.y + headerHeight + bodyHeight * (positionPercent / 100);
					return { x: typedNode.x + nodeWidth, y: outputY };
				}
			}

			return { x: node.x, y: node.y };
		},
		[layout.nodes]
	);

	return { getConnectionPoints };
}
