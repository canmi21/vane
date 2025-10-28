/* src/hooks/use-canvas-interaction.ts */

import React, { useState, useCallback, useEffect } from "react";
import { type CanvasLayout, type CanvasConnection } from "~/lib/canvas-layout";
import { nanoid } from "nanoid";

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
	// --- FIX: Allow the ref's current value to be null. ---
	canvasRef: React.RefObject<HTMLDivElement | null>;
}

/**
 * Manages all user interactions with the canvas (dragging, connecting, etc.).
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

	const handleMouseDown = useCallback(
		(e: React.MouseEvent<HTMLDivElement>) => {
			if (
				(e.button === 2 || e.button === 1) &&
				interaction.mode !== "connecting"
			) {
				e.preventDefault();
				setInteraction({
					mode: "panning",
					start: { x: e.clientX, y: e.clientY },
				});
				if (canvasRef.current) canvasRef.current.style.cursor = "grabbing";
			}
		},
		[interaction.mode, canvasRef]
	);

	const handleMouseMove = useCallback(
		(e: React.MouseEvent<HTMLDivElement>) => {
			setMousePosition({ x: e.clientX, y: e.clientY });
			if (interaction.mode === "panning") {
				const dx = e.clientX - interaction.start.x;
				const dy = e.clientY - interaction.start.y;
				panBy(dx, dy);
				setInteraction({
					...interaction,
					start: { x: e.clientX, y: e.clientY },
				});
			} else if (interaction.mode === "dragging") {
				const dx = (e.clientX - interaction.start.x) / scale;
				const dy = (e.clientY - interaction.start.y) / scale;
				const newNodes = layout.nodes.map((n) =>
					n.id === interaction.nodeId ? { ...n, x: n.x + dx, y: n.y + dy } : n
				);
				onLayoutChange({ ...layout, nodes: newNodes });
				setInteraction({
					...interaction,
					start: { x: e.clientX, y: e.clientY },
				});
			}
		},
		[interaction, scale, layout, onLayoutChange, panBy]
	);

	const handleMouseUp = useCallback(() => {
		if (interaction.mode === "panning" && canvasRef.current) {
			canvasRef.current.style.cursor = "grab";
		}
		if (interaction.mode !== "connecting") {
			setInteraction({ mode: "idle" });
		}
	}, [interaction, canvasRef]);

	const handleNodeMouseDown = useCallback(
		(nodeId: string, e: React.MouseEvent) => {
			if (
				e.button === 0 &&
				(interaction.mode === "idle" || interaction.mode === "connecting")
			) {
				e.stopPropagation();
				setInteraction({
					mode: "dragging",
					nodeId,
					start: { x: e.clientX, y: e.clientY },
				});
			}
		},
		[interaction.mode]
	);

	const handleHandleClick = useCallback(
		(nodeId: string, handleId: string) => {
			if (interaction.mode !== "connecting" && interaction.mode !== "idle")
				return;
			const handlePos = getConnectionPoints(nodeId, handleId);

			if (interaction.mode === "connecting") {
				if (interaction.fromNodeId === nodeId) return;
				const newConnection: CanvasConnection = {
					id: nanoid(),
					fromNodeId: interaction.fromNodeId,
					fromHandle: interaction.fromHandle,
					toNodeId: nodeId,
					toHandle: handleId,
				};
				onLayoutChange({
					...layout,
					connections: [...layout.connections, newConnection],
				});
				setInteraction({ mode: "idle" });
			} else {
				setInteraction({
					mode: "connecting",
					fromNodeId: nodeId,
					fromHandle: handleId,
					fromPosition: handlePos,
				});
			}
		},
		[interaction, layout, onLayoutChange, getConnectionPoints]
	);

	const handleToggleConnectorMode = () => {
		setInteraction((prev) =>
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

	const handleKeyDown = useCallback((e: KeyboardEvent) => {
		if (e.key === "Escape") setInteraction({ mode: "idle" });
	}, []);

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
		handleMouseDown,
		handleMouseMove,
		handleMouseUp,
		handleNodeMouseDown,
		handleHandleClick,
		handleToggleConnectorMode,
	};
}
