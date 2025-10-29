/* src/hooks/use-panning-and-dragging.ts */

import { useCallback, useRef } from "react";
import { type CanvasLayout } from "~/lib/canvas-layout";
import { type InteractionMode } from "./use-canvas-interaction";

interface UsePanningAndDraggingProps {
	scale: number;
	interaction: InteractionMode;
	layout: CanvasLayout;
	canvasRef: React.RefObject<HTMLDivElement | null>;
	setInteraction: (interaction: InteractionMode) => void;
	panBy: (dx: number, dy: number) => void;
	onLayoutChange: (newLayout: CanvasLayout) => void;
	setSelectedConnectionId: (id: string | null) => void;
	setSelectedNodeId: (id: string | null) => void;
	handleNodeClick: (nodeId: string) => void;
}

const CLICK_DRAG_THRESHOLD = 5; // Max pixels to move to be considered a click
const CLICK_TIME_THRESHOLD = 250; // Max milliseconds to be considered a click

/**
 * A specialized hook to manage panning the canvas and dragging nodes.
 * It now intelligently and robustly distinguishes between clicks and drags.
 */
export function usePanningAndDragging({
	scale,
	interaction,
	layout,
	canvasRef,
	setInteraction,
	panBy,
	onLayoutChange,
	setSelectedConnectionId,
	setSelectedNodeId,
	handleNodeClick,
}: UsePanningAndDraggingProps) {
	const dragStartRef = useRef<{ x: number; y: number; time: number } | null>(
		null
	);

	const handleMouseDown = useCallback(
		(e: React.MouseEvent<HTMLDivElement>) => {
			if (interaction.mode === "idle") {
				setSelectedConnectionId(null);
				setSelectedNodeId(null);
			}

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
		[
			interaction.mode,
			canvasRef,
			setInteraction,
			setSelectedConnectionId,
			setSelectedNodeId,
		]
	);

	const handleNodeMouseDown = useCallback(
		(nodeId: string, e: React.MouseEvent) => {
			if (e.button === 0 && interaction.mode === "idle") {
				e.stopPropagation();
				setSelectedConnectionId(null);
				dragStartRef.current = { x: e.clientX, y: e.clientY, time: Date.now() };

				setInteraction({
					mode: "dragging",
					nodeId,
					start: { x: e.clientX, y: e.clientY },
				});
			}
		},
		[interaction.mode, setInteraction, setSelectedConnectionId]
	);

	const handleMouseMove = useCallback(
		(e: React.MouseEvent<HTMLDivElement>) => {
			if (interaction.mode === "panning") {
				const dx = e.clientX - interaction.start.x;
				const dy = e.clientY - interaction.start.y;
				panBy(dx, dy);
				setInteraction({
					...interaction,
					start: { x: e.clientX, y: e.clientY },
				});
			} else if (interaction.mode === "dragging") {
				// Only apply movement if it's a real drag, determined by distance/time on mouseUp
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
		[interaction, scale, layout, onLayoutChange, panBy, setInteraction]
	);

	const handleMouseUp = useCallback(
		(nodeId?: string) => {
			if (interaction.mode === "dragging" && dragStartRef.current && nodeId) {
				const { x, y, time } = dragStartRef.current;
				const endX = (interaction.start as { x: number }).x;
				const endY = (interaction.start as { y: number }).y;

				const dx = Math.abs(endX - x);
				const dy = Math.abs(endY - y);
				const timeElapsed = Date.now() - time;

				if (
					dx < CLICK_DRAG_THRESHOLD &&
					dy < CLICK_DRAG_THRESHOLD &&
					timeElapsed < CLICK_TIME_THRESHOLD
				) {
					handleNodeClick(nodeId);
				}
			}

			dragStartRef.current = null;

			if (interaction.mode === "panning" && canvasRef.current) {
				canvasRef.current.style.cursor = "grab";
			}
			if (interaction.mode === "panning" || interaction.mode === "dragging") {
				setInteraction({ mode: "idle" });
			}
		},
		[interaction, canvasRef, setInteraction, handleNodeClick]
	);

	return {
		handleMouseDown,
		handleNodeMouseDown,
		handleMouseMove,
		handleMouseUp,
	};
}
