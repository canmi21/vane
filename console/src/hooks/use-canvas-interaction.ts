/* src/hooks/use-canvas-interaction.ts */

import React, { useState, useCallback, useEffect } from "react";
import { type CanvasLayout } from "~/lib/canvas-layout";
import { usePanningAndDragging } from "./use-panning-and-dragging";
import { useConnectionManagement } from "./use-connection-management";

// --- Types ---
export type InteractionMode =
	| { mode: "idle" }
	| { mode: "panning"; start: { x: number; y: number } }
	| { mode: "dragging"; nodeId: string; start: { x: number; y: number } }
	| {
			mode: "connecting";
			fromNodeId: string;
			fromHandle: string;
			fromPosition: { x: number; y: number };
	  };

interface UseCanvasInteractionProps {
	scale: number;
	view: { x: number; y: number };
	layout: CanvasLayout;
	onLayoutChange: (newLayout: CanvasLayout) => void;
	panBy: (dx: number, dy: number) => void;
	getConnectionPoints: (
		nodeId: string,
		handleId: string
	) => { x: number; y: number };
	canvasRef: React.RefObject<HTMLDivElement | null>;
}

/**
 * A coordinator hook that manages all user interactions by composing specialized sub-hooks.
 */
export function useCanvasInteraction({
	scale,
	view,
	layout,
	onLayoutChange,
	panBy,
	getConnectionPoints,
	canvasRef,
}: UseCanvasInteractionProps) {
	const [interaction, setInteraction] = useState<InteractionMode>({
		mode: "idle",
	});
	const [mousePosition, setMousePosition] = useState({ x: 0, y: 0 });
	const [selectedConnectionId, setSelectedConnectionId] = useState<
		string | null
	>(null);

	const panningAndDragging = usePanningAndDragging({
		scale,
		interaction,
		layout,
		canvasRef,
		setInteraction,
		panBy,
		onLayoutChange,
		setSelectedConnectionId,
	});

	const connectionManagement = useConnectionManagement({
		interaction,
		layout,
		selectedConnectionId,
		setInteraction,
		setSelectedConnectionId,
		onLayoutChange,
		getConnectionPoints,
	});

	// Some logic, like mouse move for the preview line, remains here
	const handleOverallMouseMove = useCallback(
		(e: React.MouseEvent<HTMLDivElement>) => {
			panningAndDragging.handleMouseMove(e);

			if (canvasRef.current) {
				const canvasRect = canvasRef.current.getBoundingClientRect();
				setMousePosition({
					x: e.clientX - canvasRect.left,
					y: e.clientY - canvasRect.top,
				});
			}
		},
		[panningAndDragging, canvasRef]
	);

	const handleKeyDown = useCallback(
		(e: KeyboardEvent) => {
			if (e.key === "Escape") {
				setInteraction({ mode: "idle" });
				setSelectedConnectionId(null);
			}
			if (
				(e.key === "Backspace" || e.key === "Delete") &&
				selectedConnectionId
			) {
				connectionManagement.handleDeleteSelectedConnection();
			}
		},
		[selectedConnectionId, connectionManagement]
	);

	useEffect(() => {
		window.addEventListener("keydown", handleKeyDown);
		return () => window.removeEventListener("keydown", handleKeyDown);
	}, [handleKeyDown]);

	const mouseInCanvasCoords = {
		x: (mousePosition.x - view.x) / scale,
		y: (mousePosition.y - view.y) / scale,
	};

	return {
		interaction,
		mouseInCanvasCoords,
		selectedConnectionId,
		handleMouseDown: panningAndDragging.handleMouseDown,
		handleMouseMove: handleOverallMouseMove,
		handleMouseUp: panningAndDragging.handleMouseUp,
		handleNodeMouseDown: panningAndDragging.handleNodeMouseDown,
		handleHandleClick: connectionManagement.handleHandleClick,
		handleConnectionClick: connectionManagement.handleConnectionClick,
		handleDeleteSelectedConnection:
			connectionManagement.handleDeleteSelectedConnection,
		handleToggleConnectorMode: connectionManagement.handleToggleConnectorMode,
	};
}
