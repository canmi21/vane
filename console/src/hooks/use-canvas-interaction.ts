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

	// --- FINAL FIX: Mouse position is now calculated relative to the canvas element itself. ---
	const handleMouseMove = useCallback(
		(e: React.MouseEvent<HTMLDivElement>) => {
			// Panning and dragging rely on deltas from the raw clientX/Y, so they are handled first.
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

			// For calculating the preview line's end point, we need the precise relative coordinate.
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

	// --- FINAL FIX: Rewritten with robust validation for creating connections. ---
	const handleHandleClick = useCallback(
		(nodeId: string, handleId: string) => {
			const targetNode = layout.nodes.find((n) => n.id === nodeId);
			if (!targetNode) return;

			// --- Case 1: FINISHING a connection ---
			if (interaction.mode === "connecting") {
				const startNode = layout.nodes.find(
					(n) => n.id === interaction.fromNodeId
				);
				// The startNode should always exist if we are in connecting mode from a handle click.
				if (!startNode) {
					setInteraction({ mode: "idle" });
					return;
				}

				// --- Validation Checks ---
				// Rule 1: Cannot connect a node to itself.
				if (startNode.id === targetNode.id) return;
				// Rule 2: Must connect an output to an input.
				const isStartHandleOutput = startNode.outputs.some(
					(h) => h.id === interaction.fromHandle
				);
				const isTargetHandleInput = targetNode.inputs.some(
					(h) => h.id === handleId
				);
				if (!isStartHandleOutput || !isTargetHandleInput) return;
				// Rule 3: Target input handle must not be occupied.
				if (
					layout.connections.some(
						(c) => c.toNodeId === nodeId && c.toHandle === handleId
					)
				)
					return;
				// Rule 4: Source output handle must not be occupied.
				if (
					layout.connections.some(
						(c) =>
							c.fromNodeId === startNode.id &&
							c.fromHandle === interaction.fromHandle
					)
				)
					return;

				// All checks passed. Create the new connection.
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

				// --- Case 2: STARTING a new connection ---
			} else if (interaction.mode === "idle") {
				// Can only start a connection from an OUTPUT handle.
				const isStartHandleOutput = targetNode.outputs.some(
					(h) => h.id === handleId
				);
				if (!isStartHandleOutput) return;

				// A connection can't start from an already connected output handle.
				if (
					layout.connections.some(
						(c) => c.fromNodeId === nodeId && c.fromHandle === handleId
					)
				)
					return;

				const handlePos = getConnectionPoints(nodeId, handleId);
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
