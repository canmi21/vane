/* src/hooks/use-canvas-view.ts */

import { useState, useCallback, useEffect } from "react";
import { type CanvasNode } from "~/lib/canvas-layout";
import { type Plugin } from "./use-plugin-data"; // Import Plugin type

// --- Constants ---
const ZOOM_SENSITIVITY = 0.001;
const MIN_SCALE = 0.2;
const MAX_SCALE = 3;
const NODE_WIDTH = 256;
const HEADER_HEIGHT = 41;

interface UseCanvasViewProps {
	canvasRef: React.RefObject<HTMLDivElement | null>;
	nodes: CanvasNode[];
	plugins: Plugin[];
}

/**
 * Manages the view state (pan, zoom) of the canvas.
 */
export function useCanvasView({
	canvasRef,
	nodes,
	plugins,
}: UseCanvasViewProps) {
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
			let nodeHeight: number;
			if (node.type === "entry-point") {
				nodeHeight = 83; // Entry point has a fixed height
			} else {
				const heightFromOutputs =
					HEADER_HEIGHT * (node.outputs.length > 0 ? node.outputs.length : 1);
				let inputParamCount = 0;
				if (node.type === "error-page") {
					inputParamCount = 9; // Hardcoded count for error-page node
				}
				// --- FINAL FIX: Add a case for the new node type's height calculation. ---
				else if (node.type === "return-response") {
					inputParamCount = 3; // It has 3 parameters
				} else {
					const plugin = plugins.find((p) => p.name === node.type);
					inputParamCount = plugin
						? Object.keys(plugin.input_params).length
						: 0;
				}
				const ESTIMATED_INPUT_ROW_HEIGHT = 60;
				const TOP_PADDING = 24;
				const BOTTOM_PADDING = 12;
				const heightFromInputs =
					inputParamCount * ESTIMATED_INPUT_ROW_HEIGHT +
					TOP_PADDING +
					BOTTOM_PADDING;
				const bodyHeight = Math.max(heightFromOutputs, heightFromInputs);
				nodeHeight = HEADER_HEIGHT + bodyHeight;
			}

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
	}, [nodes, canvasRef, plugins]);

	const handleResetView = useCallback(() => {
		setView({ x: 0, y: 0 });
		setScale(1);
	}, []);

	return { view, scale, panBy, handleFitView, handleResetView };
}
