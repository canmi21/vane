/* src/hooks/use-panning-and-dragging.ts */

import { useCallback, useRef, useEffect } from "react";
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

const CLICK_DRAG_THRESHOLD = 5;
const CLICK_TIME_THRESHOLD = 250;

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
	// --- FINAL FIX: Use a ref to hold the latest interaction state to prevent stale closures ---
	const interactionRef = useRef(interaction);
	useEffect(() => {
		interactionRef.current = interaction;
	}, [interaction]);

	const handleMouseDown = useCallback(
		(e: React.MouseEvent<HTMLDivElement>) => {
			// Read from the ref to get the most up-to-date state
			if (interactionRef.current.mode === "idle") {
				if (e.button === 0) {
					setSelectedConnectionId(null);
					setSelectedNodeId(null);
				} else if (e.button === 2 || e.button === 1) {
					e.preventDefault();
					setInteraction({
						mode: "panning",
						start: { x: e.clientX, y: e.clientY },
					});
					if (canvasRef.current) canvasRef.current.style.cursor = "grabbing";
				}
			}
		},
		// This function now has stable dependencies and will not be recreated unnecessarily.
		[canvasRef, setInteraction, setSelectedConnectionId, setSelectedNodeId]
	);

	const handleNodeMouseDown = useCallback(
		(nodeId: string, e: React.MouseEvent) => {
			if (e.button === 0 && interactionRef.current.mode === "idle") {
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
		[setInteraction, setSelectedConnectionId]
	);

	const handleMouseMove = useCallback(
		(e: React.MouseEvent<HTMLDivElement>) => {
			// Read the LATEST state directly from the ref inside the event handler
			const currentInteraction = interactionRef.current;

			if (currentInteraction.mode === "panning") {
				const dx = e.clientX - currentInteraction.start.x;
				const dy = e.clientY - currentInteraction.start.y;
				panBy(dx, dy);
				setInteraction({
					...currentInteraction,
					start: { x: e.clientX, y: e.clientY },
				});
			} else if (currentInteraction.mode === "dragging") {
				const dx = (e.clientX - currentInteraction.start.x) / scale;
				const dy = (e.clientY - currentInteraction.start.y) / scale;
				const newNodes = layout.nodes.map((n) =>
					n.id === currentInteraction.nodeId
						? { ...n, x: n.x + dx, y: n.y + dy }
						: n
				);
				onLayoutChange({ ...layout, nodes: newNodes });
				setInteraction({
					...currentInteraction,
					start: { x: e.clientX, y: e.clientY },
				});
			}
		},
		// This function is now stable and does not depend on the `interaction` prop.
		[scale, layout, onLayoutChange, panBy, setInteraction]
	);

	const handleMouseUp = useCallback(
		(nodeId?: string) => {
			const currentInteraction = interactionRef.current;
			if (
				currentInteraction.mode === "dragging" &&
				dragStartRef.current &&
				nodeId
			) {
				const { x, y, time } = dragStartRef.current;
				const endX = (currentInteraction.start as { x: number }).x;
				const endY = (currentInteraction.start as { y: number }).y;

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

			if (currentInteraction.mode === "panning" && canvasRef.current) {
				canvasRef.current.style.cursor = "grab";
			}
			if (
				currentInteraction.mode === "panning" ||
				currentInteraction.mode === "dragging"
			) {
				setInteraction({ mode: "idle" });
			}
		},
		[canvasRef, setInteraction, handleNodeClick]
	);

	return {
		handleMouseDown,
		handleNodeMouseDown,
		handleMouseMove,
		handleMouseUp,
	};
}
