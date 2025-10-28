/* src/hooks/use-canvas-view.ts */

import { useState, useCallback, useEffect } from "react";
import { type CanvasNode } from "~/lib/canvas-layout";

// --- Constants ---
const ZOOM_SENSITIVITY = 0.001;
const MIN_SCALE = 0.2;
const MAX_SCALE = 3;
const NODE_WIDTH = 256;
const NODE_HEIGHTS: Record<string, number> = {
	"entry-point": 83,
	"rate-limit": 123,
};
const DEFAULT_NODE_HEIGHT = 100;

interface UseCanvasViewProps {
	// --- FIX: Allow the ref's current value to be null, matching how useRef works. ---
	canvasRef: React.RefObject<HTMLDivElement | null>;
	nodes: CanvasNode[];
}

/**
 * Manages the view state (pan, zoom) of the canvas.
 */
export function useCanvasView({ canvasRef, nodes }: UseCanvasViewProps) {
	const [view, setView] = useState({ x: 0, y: 0 });
	const [scale, setScale] = useState(1);

	// --- Pan & Zoom Logic ---
	const panBy = useCallback((dx: number, dy: number) => {
		setView((v) => ({ x: v.x + dx, y: v.y + dy }));
	}, []);

	const handleWheel = useCallback(
		(e: WheelEvent) => {
			e.preventDefault();
			if (e.ctrlKey) {
				const zoomAmount = e.deltaY * -ZOOM_SENSITIVITY;
				setScale((prevScale) =>
					Math.min(Math.max(prevScale + zoomAmount, MIN_SCALE), MAX_SCALE)
				);
			} else {
				panBy(-e.deltaX, -e.deltaY);
			}
		},
		[panBy]
	);

	useEffect(() => {
		const canvasElement = canvasRef.current;
		if (canvasElement) {
			canvasElement.addEventListener("wheel", handleWheel, { passive: false });
			return () => canvasElement.removeEventListener("wheel", handleWheel);
		}
	}, [canvasRef, handleWheel]);

	// --- View Control Actions ---
	const handleFitView = useCallback(() => {
		if (!canvasRef.current || nodes.length === 0) return;
		const { width: canvasWidth, height: canvasHeight } =
			canvasRef.current.getBoundingClientRect();

		let minX = Infinity,
			minY = Infinity,
			maxX = -Infinity,
			maxY = -Infinity;

		nodes.forEach((node) => {
			const nodeHeight = NODE_HEIGHTS[node.type] ?? DEFAULT_NODE_HEIGHT;
			minX = Math.min(minX, node.x);
			minY = Math.min(minY, node.y);
			maxX = Math.max(maxX, node.x + NODE_WIDTH);
			maxY = Math.max(maxY, node.y + nodeHeight);
		});

		const boxWidth = maxX - minX;
		const boxHeight = maxY - minY;
		if (boxWidth === 0 || boxHeight === 0) return;

		const PADDING = 120;
		const scaleX = (canvasWidth - PADDING) / boxWidth;
		const scaleY = (canvasHeight - PADDING) / boxHeight;
		const newScale = Math.min(scaleX, scaleY, MAX_SCALE);
		const clampedScale = Math.max(MIN_SCALE, newScale);

		const boxCenterX = minX + boxWidth / 2;
		const boxCenterY = minY + boxHeight / 2;
		const newViewX = canvasWidth / 2 - boxCenterX * clampedScale;
		const newViewY = canvasHeight / 2 - boxCenterY * clampedScale;

		setView({ x: newViewX, y: newViewY });
		setScale(clampedScale);
	}, [nodes, canvasRef]);

	const handleResetView = useCallback(() => {
		setView({ x: 0, y: 0 });
		setScale(1);
	}, []);

	return { view, scale, panBy, handleFitView, handleResetView };
}
