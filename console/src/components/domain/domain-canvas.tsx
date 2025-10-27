/* src/components/domain/domain-canvas.tsx */

import React, { useState, useRef, useCallback } from "react";
import { motion } from "framer-motion";
import { CanvasToolbar } from "./canvas-toolbar";

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
	const [view, setView] = useState({ x: 0, y: 0 });
	const [scale, setScale] = useState(1);
	const isPanning = useRef(false);
	const lastMousePosition = useRef({ x: 0, y: 0 });

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

	const handleWheel = useCallback((e: React.WheelEvent) => {
		e.preventDefault();

		if (e.ctrlKey) {
			const zoomAmount = e.deltaY * -ZOOM_SENSITIVITY;
			setScale((prevScale) =>
				Math.min(Math.max(prevScale + zoomAmount, MIN_SCALE), MAX_SCALE)
			);
		} else {
			const deltaX = e.deltaX;
			const deltaY = e.deltaY;
			setView((prevView) => ({
				x: prevView.x - deltaX,
				y: prevView.y - deltaY,
			}));
		}
	}, []);

	const resetView = useCallback(() => {
		setView({ x: 0, y: 0 });
		setScale(1);
	}, []);

	const handleContextMenu = useCallback((e: React.MouseEvent) => {
		e.preventDefault();
	}, []);

	return {
		view,
		scale,
		resetView,
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
		resetView,
		handleMouseDown,
		handleMouseUp,
		handleMouseMove,
		handleMouseLeave,
		handleContextMenu,
		handleWheel,
	} = useCanvasGestures(canvasRef);

	const backgroundStyle: React.CSSProperties = {
		"--grid-line-minor-color": "var(--color-bg-alt)",
		"--grid-line-major-color": "var(--scrollbar-thumb)",
		backgroundImage: `
			linear-gradient(var(--grid-line-major-color) 1px, transparent 1px),
			linear-gradient(to right, var(--grid-line-major-color) 1px, transparent 1px),
			linear-gradient(var(--grid-line-minor-color) 1px, transparent 1px),
			linear-gradient(to right, var(--grid-line-minor-color) 1px, transparent 1px)
		`,
		backgroundSize: `
			${100 * scale}px ${100 * scale}px,
			${100 * scale}px ${100 * scale}px,
			${20 * scale}px ${20 * scale}px,
			${20 * scale}px ${20 * scale}px
		`,
		backgroundPosition: `${view.x}px ${view.y}px`,
	} as React.CSSProperties;

	return (
		<div
			ref={canvasRef}
			className="h-full w-full cursor-grab overflow-hidden bg-[var(--color-bg)]"
			style={backgroundStyle}
			onMouseDown={handleMouseDown}
			onMouseUp={handleMouseUp}
			onMouseMove={handleMouseMove}
			onMouseLeave={handleMouseLeave}
			onContextMenu={handleContextMenu}
			onWheel={handleWheel}
		>
			<CanvasToolbar onResetView={resetView} />

			<motion.div
				className="absolute inset-0 flex items-center justify-center"
				// --- FIX: Move x and y to `style` for instant updates ---
				style={{
					x: view.x,
					y: view.y,
				}}
				// --- Keep `scale` in `animate` for smooth transitions ---
				animate={{
					scale: scale,
				}}
				// The transition now only applies to properties in `animate` (i.e., scale)
				transition={{
					type: "spring",
					stiffness: 400,
					damping: 40,
				}}
			>
				{children}
			</motion.div>
		</div>
	);
}
