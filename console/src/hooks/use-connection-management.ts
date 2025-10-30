/* src/hooks/use-connection-management.ts */

import { useCallback } from "react";
import { nanoid } from "nanoid";
import { type CanvasLayout, type CanvasConnection } from "~/lib/canvas-layout";
import { type InteractionMode } from "./use-canvas-interaction";

interface UseConnectionManagementProps {
	interaction: InteractionMode;
	layout: CanvasLayout;
	selectedConnectionId: string | null;
	setInteraction: (
		interaction: InteractionMode | ((prev: InteractionMode) => InteractionMode)
	) => void;
	setSelectedConnectionId: (id: string | null) => void;
	// --- FINAL FIX: Add the missing prop to the interface type ---
	setSelectedNodeId: (id: string | null) => void;
	onLayoutChange: (newLayout: CanvasLayout) => void;
	getConnectionPoints: (
		nodeId: string,
		handleId: string
	) => { x: number; y: number };
}

/**
 * A specialized hook to manage creating, selecting, and deleting connections.
 */
export function useConnectionManagement({
	interaction,
	layout,
	selectedConnectionId,
	setInteraction,
	setSelectedConnectionId,
	setSelectedNodeId,
	onLayoutChange,
	getConnectionPoints,
}: UseConnectionManagementProps) {
	const handleHandleClick = useCallback(
		(nodeId: string, handleId: string) => {
			setSelectedConnectionId(null);
			setSelectedNodeId(null);
			const clickedNode = layout.nodes.find((n) => n.id === nodeId);
			if (!clickedNode) return;

			if (interaction.mode === "connecting") {
				const startNode = layout.nodes.find(
					(n) => n.id === interaction.fromNodeId
				);

				const isValidTarget =
					startNode &&
					startNode.id !== clickedNode.id &&
					clickedNode.inputs.some((h) => h.id === handleId) &&
					!layout.connections.some(
						(c) => c.toNodeId === clickedNode.id && c.toHandle === handleId
					);

				if (isValidTarget) {
					const newConnection: CanvasConnection = {
						id: nanoid(),
						fromNodeId: interaction.fromNodeId,
						fromHandle: interaction.fromHandle,
						toNodeId: clickedNode.id,
						toHandle: handleId,
					};
					onLayoutChange({
						...layout,
						connections: [...layout.connections, newConnection],
					});
					setInteraction({ mode: "idle" });
				}
				return;
			}

			if (interaction.mode === "idle") {
				const isOutput = clickedNode.outputs.some((h) => h.id === handleId);
				const isOccupied = layout.connections.some(
					(c) => c.fromNodeId === clickedNode.id && c.fromHandle === handleId
				);

				if (isOutput && !isOccupied) {
					const handlePos = getConnectionPoints(clickedNode.id, handleId);
					setInteraction({
						mode: "connecting",
						fromNodeId: clickedNode.id,
						fromHandle: handleId,
						fromPosition: handlePos,
					});
				}
			}
		},
		[
			interaction,
			layout,
			onLayoutChange,
			getConnectionPoints,
			setInteraction,
			setSelectedConnectionId,
			setSelectedNodeId,
		]
	);

	const handleConnectionClick = useCallback(
		(connectionId: string) => {
			if (interaction.mode === "idle") {
				setSelectedNodeId(null);
				setSelectedConnectionId(connectionId);
			}
		},
		[interaction.mode, setSelectedConnectionId, setSelectedNodeId]
	);

	const handleDeleteSelectedConnection = useCallback(() => {
		if (!selectedConnectionId) return;
		const newConnections = layout.connections.filter(
			(c) => c.id !== selectedConnectionId
		);
		onLayoutChange({ ...layout, connections: newConnections });
		setSelectedConnectionId(null);
	}, [selectedConnectionId, layout, onLayoutChange, setSelectedConnectionId]);

	const handleToggleConnectorMode = () => {
		setSelectedConnectionId(null);
		setSelectedNodeId(null);
		setInteraction((prev: InteractionMode) =>
			prev.mode === "connecting"
				? { mode: "idle" }
				: {
						mode: "connecting",
						fromNodeId: "",
						fromHandle: "",
						fromPosition: { x: 0, y: 0 },
					}
		);
	};

	return {
		handleHandleClick,
		handleConnectionClick,
		handleDeleteSelectedConnection,
		handleToggleConnectorMode,
	};
}
