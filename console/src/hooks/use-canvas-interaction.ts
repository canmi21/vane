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
			if (interaction.mode === "panning") {
				const dx = e.clientX - interaction.start.x;
				const dy = e.clientY - interaction.start.y;
				panBy(dx, dy);
				setInteraction(
					(prev) =>
						({
							...prev,
							start: { x: e.clientX, y: e.clientY },
						}) as InteractionMode
				);
			} else if (interaction.mode === "dragging") {
				const dx = (e.clientX - interaction.start.x) / scale;
				const dy = (e.clientY - interaction.start.y) / scale;
				const newNodes = layout.nodes.map((n) =>
					n.id === interaction.nodeId ? { ...n, x: n.x + dx, y: n.y + dy } : n
				);
				onLayoutChange({ ...layout, nodes: newNodes });
				setInteraction(
					(prev) =>
						({
							...prev,
							start: { x: e.clientX, y: e.clientY },
						}) as InteractionMode
				);
			}

			if (canvasRef.current) {
				const canvasRect = canvasRef.current.getBoundingClientRect();
				setMousePosition({
					x: e.clientX - canvasRect.left,
					y: e.clientY - canvasRect.top,
				});
			}
		},
		[interaction, scale, layout, onLayoutChange, panBy, canvasRef]
	);

	const handleMouseUp = useCallback(() => {
		if (interaction.mode === "panning" && canvasRef.current) {
			canvasRef.current.style.cursor = "grab";
		}
		// Only reset to idle if we were panning or dragging. This prevents interference with clicks.
		if (interaction.mode === "panning" || interaction.mode === "dragging") {
			setInteraction({ mode: "idle" });
		}
	}, [interaction, canvasRef]);

	const handleNodeMouseDown = useCallback(
		(nodeId: string, e: React.MouseEvent) => {
			// If we are trying to connect, a mousedown on a node should not start a drag.
			// Let the subsequent `onClick` on the handle do its job.
			if (e.button === 0 && interaction.mode === "idle") {
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
				// If the target is not valid, we do nothing and stay in connecting mode,
				// allowing the user to try another target.
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
