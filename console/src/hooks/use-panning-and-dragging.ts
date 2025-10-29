/* src/hooks/use-panning-and-dragging.ts */

import { useCallback } from "react";
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
}

/**
 * A specialized hook to manage panning the canvas and dragging nodes.
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
}: UsePanningAndDraggingProps) {
	const handleMouseDown = useCallback(
		(e: React.MouseEvent<HTMLDivElement>) => {
			if (interaction.mode === "idle") {
				setSelectedConnectionId(null);
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
		[interaction.mode, canvasRef, setInteraction, setSelectedConnectionId]
	);

	const handleNodeMouseDown = useCallback(
		(nodeId: string, e: React.MouseEvent) => {
			if (e.button === 0 && interaction.mode === "idle") {
				e.stopPropagation();
				setSelectedConnectionId(null);
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

	const handleMouseUp = useCallback(() => {
		if (interaction.mode === "panning" && canvasRef.current) {
			canvasRef.current.style.cursor = "grab";
		}
		if (interaction.mode === "panning" || interaction.mode === "dragging") {
			setInteraction({ mode: "idle" });
		}
	}, [interaction.mode, canvasRef, setInteraction]);

	return {
		handleMouseDown,
		handleNodeMouseDown,
		handleMouseMove,
		handleMouseUp,
	};
}
