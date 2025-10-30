/* src/hooks/use-node-management.ts */

import { useCallback } from "react";
import { type CanvasLayout } from "~/lib/canvas-layout";

interface UseNodeManagementProps {
	layout: CanvasLayout;
	selectedNodeId: string | null;
	setSelectedNodeId: (id: string | null) => void;
	onLayoutChange: (newLayout: CanvasLayout) => void;
}

/**
 * A specialized hook to manage selecting and deleting nodes.
 */
export function useNodeManagement({
	layout,
	selectedNodeId,
	setSelectedNodeId,
	onLayoutChange,
}: UseNodeManagementProps) {
	const handleNodeClick = useCallback(
		(nodeId: string) => {
			setSelectedNodeId(nodeId);
		},
		[setSelectedNodeId]
	);

	const handleDeleteSelectedNode = useCallback(() => {
		// Rule: The entry-point node cannot be deleted.
		if (!selectedNodeId || selectedNodeId === "entry-point") return;

		// Filter out the selected node
		const newNodes = layout.nodes.filter((n) => n.id !== selectedNodeId);

		// Filter out any connections attached to the selected node (chain reaction)
		const newConnections = layout.connections.filter(
			(c) => c.fromNodeId !== selectedNodeId && c.toNodeId !== selectedNodeId
		);

		onLayoutChange({ nodes: newNodes, connections: newConnections });
		setSelectedNodeId(null);
	}, [selectedNodeId, layout, onLayoutChange, setSelectedNodeId]);

	return { handleNodeClick, handleDeleteSelectedNode };
}
