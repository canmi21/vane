/* src/components/domain/domain-canvas.tsx */

import React, { useState, useRef, useCallback } from "react";
import { motion } from "framer-motion";

// --- Constants ---
const ZOOM_SENSITIVITY = 0.001;
const MIN_SCALE = 0.2;
const MAX_SCALE = 3;

// --- Hooks ---
/**
 * A custom hook to handle canvas panning and zooming gestures.
 * @param canvasRef - A React ref attached to the canvas element.
 */
function useCanvasGestures(canvasRef: React.RefObject<HTMLDivElement | null>) {
	// --- State ---
	const [view, setView] = useState({ x: 0, y: 0 });
	const [scale, setScale] = useState(1);
	const isPanning = useRef(false);
	const lastMousePosition = useRef({ x: 0, y: 0 });

	// --- Mouse Panning (Right-click drag) ---
	const handleMouseDown = useCallback(
		(e: React.MouseEvent) => {
			if (e.button === 2 && canvasRef.current) {
				isPanning.current = true;
				lastMousePosition.current = { x: e.clientX, y: e.clientY };
				canvasRef.current.style.cursor = "grabbing";
				e.preventDefault();
			}
		},
		[canvasRef]
	);

	const handleMouseUp = useCallback(
		(e: React.MouseEvent) => {
			if (e.button === 2 && canvasRef.current) {
				isPanning.current = false;
				canvasRef.current.style.cursor = "grab";
			}
		},
		[canvasRef]
	);

	const handleMouseMove = useCallback((e: React.MouseEvent) => {
		if (!isPanning.current) return;
		const deltaX = e.clientX - lastMousePosition.current.x;
		const deltaY = e.clientY - lastMousePosition.current.y;
		lastMousePosition.current = { x: e.clientX, y: e.clientY };
		setView((prev) => ({ x: prev.x + deltaX, y: prev.y + deltaY }));
	}, []);

	const handleMouseLeave = useCallback(() => {
		if (isPanning.current && canvasRef.current) {
			isPanning.current = false;
			canvasRef.current.style.cursor = "grab";
		}
	}, [canvasRef]);

	// --- Trackpad & Mouse Wheel Gestures ---
	const handleWheel = useCallback((e: React.WheelEvent) => {
		e.preventDefault(); // Prevent page scroll

		// Pinch-to-zoom gesture (detected with ctrlKey on many trackpads/mice)
		if (e.ctrlKey) {
			const zoomAmount = e.deltaY * -ZOOM_SENSITIVITY;
			setScale((prevScale) =>
				// Clamp the scale between min and max values
				Math.min(Math.max(prevScale + zoomAmount, MIN_SCALE), MAX_SCALE)
			);
		} else {
			// Two-finger swipe to pan
			const deltaX = e.deltaX;
			const deltaY = e.deltaY;
			setView((prevView) => ({
				x: prevView.x - deltaX,
				y: prevView.y - deltaY,
			}));
		}
	}, []);

	const handleContextMenu = useCallback((e: React.MouseEvent) => {
		e.preventDefault();
	}, []);

	return {
		view,
		scale,
		handleMouseDown,
		handleMouseUp,
		handleMouseMove,
		handleMouseLeave,
		handleContextMenu,
		handleWheel,
	};
}

// --- Component ---
export function DomainCanvas({ children }: { children?: React.ReactNode }) {
	const canvasRef = React.useRef<HTMLDivElement>(null);
	const {
		view,
		scale,
		handleMouseDown,
		handleMouseUp,
		handleMouseMove,
		handleMouseLeave,
		handleContextMenu,
		handleWheel,
	} = useCanvasGestures(canvasRef);

	const backgroundStyle: React.CSSProperties = {
		// Define CSS variables for grid line colors
		"--grid-line-minor-color": "var(--color-bg-alt)",
		// --- MODIFIED: Changed to a more subtle color for the major grid lines ---
		"--grid-line-major-color": "var(--scrollbar-thumb)",

		// Layer multiple backgrounds. The first image is on top.
		// 1. Major grid (more prominent)
		// 2. Minor grid (less prominent)
		backgroundImage: `
			linear-gradient(var(--grid-line-major-color) 1px, transparent 1px),
			linear-gradient(to right, var(--grid-line-major-color) 1px, transparent 1px),
			linear-gradient(var(--grid-line-minor-color) 1px, transparent 1px),
			linear-gradient(to right, var(--grid-line-minor-color) 1px, transparent 1px)
		`,
		// Define the size for each corresponding background image
		backgroundSize: `
			${100 * scale}px ${100 * scale}px,
			${100 * scale}px ${100 * scale}px,
			${20 * scale}px ${20 * scale}px,
			${20 * scale}px ${20 * scale}px
		`,
		// A single position moves all layers together
		backgroundPosition: `${view.x}px ${view.y}px`,
	} as React.CSSProperties;

	return (
		<div
			ref={canvasRef}
			className="h-full w-full cursor-grab overflow-hidden bg-[var(--color-bg)]"
			style={backgroundStyle} // Apply the new combined style
			onMouseDown={handleMouseDown}
			onMouseUp={handleMouseUp}
			onMouseMove={handleMouseMove}
			onMouseLeave={handleMouseLeave}
			onContextMenu={handleContextMenu}
			onWheel={handleWheel}
		>
			<motion.div
				className="absolute"
				style={{
					left: view.x,
					top: view.y,
					scale,
				}}
			>
				{children}
			</motion.div>
		</div>
	);
}
