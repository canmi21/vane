/* src/components/domain/domain-canvas.tsx */

import React, { useState, useRef, useCallback, useEffect } from "react";
import { motion, AnimatePresence } from "framer-motion";
import { CanvasToolbar } from "./canvas-toolbar";
import { type CanvasLayout, type CanvasConnection } from "~/lib/canvas-layout";
import {
	DomainEntryPointCard,
	type NodeComponentProps,
} from "./domain-entry-point-card";
import { RateLimitCard } from "./rate-limit-card";
import { CanvasConnector } from "./canvas-connector";
import { nanoid } from "nanoid";

// --- Constants & Types ---
const ZOOM_SENSITIVITY = 0.001;
const MIN_SCALE = 0.2;
const MAX_SCALE = 3;
const NODE_WIDTH = 256;
const NODE_HEIGHTS: Record<string, number> = {
	"entry-point": 83,
	"rate-limit": 123,
};
const DEFAULT_NODE_HEIGHT = 100;

type InteractionMode =
	| { mode: "idle" }
	| { mode: "panning"; start: { x: number; y: number } }
	| { mode: "dragging"; nodeId: string; start: { x: number; y: number } }
	| {
			mode: "connecting";
			fromNodeId: string;
			fromHandle: string;
			fromPosition: { x: number; y: number }; // Canvas coordinates
	  };

interface DomainCanvasProps {
	layout: CanvasLayout;
	onLayoutChange: (newLayout: CanvasLayout) => void;
	selectedDomain: string;
}

// --- Main Canvas Component ---
export function DomainCanvas({
	layout,
	onLayoutChange,
	selectedDomain,
}: DomainCanvasProps) {
	const canvasRef = useRef<HTMLDivElement>(null);
	const [view, setView] = useState({ x: 0, y: 0 });
	const [scale, setScale] = useState(1);
	const [interaction, setInteraction] = useState<InteractionMode>({
		mode: "idle",
	});
	const [mousePosition, setMousePosition] = useState({ x: 0, y: 0 });

	const getConnectionPoints = useCallback(
		(nodeId: string, handleId: string): { x: number; y: number } => {
			const node = layout.nodes.find((n) => n.id === nodeId);
			if (!node) return { x: 0, y: 0 };

			const nodeWidth = 256;
			const entryPointHeight = 83;
			const rateLimitHeight = 123;

			if (node.type === "entry-point") {
				return {
					x: node.x + nodeWidth,
					y: node.y + entryPointHeight / 2,
				};
			}
			if (node.type === "rate-limit" && handleId === "input") {
				return { x: node.x, y: node.y + rateLimitHeight / 2 };
			}
			return { x: node.x, y: node.y };
		},
		[layout.nodes]
	);

	// --- ADDED: The logic for the "Fit to View" feature ---
	const handleFitView = useCallback(() => {
		if (!canvasRef.current || layout.nodes.length === 0) {
			setView({ x: 0, y: 0 });
			setScale(1);
			return;
		}

		const canvasElement = canvasRef.current;
		const { width: canvasWidth, height: canvasHeight } =
			canvasElement.getBoundingClientRect();

		let minX = Infinity,
			minY = Infinity,
			maxX = -Infinity,
			maxY = -Infinity;

		layout.nodes.forEach((node) => {
			const nodeHeight = NODE_HEIGHTS[node.type] ?? DEFAULT_NODE_HEIGHT;
			minX = Math.min(minX, node.x);
			minY = Math.min(minY, node.y);
			maxX = Math.max(maxX, node.x + NODE_WIDTH);
			maxY = Math.max(maxY, node.y + nodeHeight);
		});

		const boxWidth = maxX - minX;
		const boxHeight = maxY - minY;

		if (boxWidth === 0 || boxHeight === 0) return;

		const PADDING = 120; // Pixels of padding
		const scaleX = (canvasWidth - PADDING) / boxWidth;
		const scaleY = (canvasHeight - PADDING) / boxHeight;
		const newScale = Math.min(scaleX, scaleY);
		const clampedScale = Math.max(MIN_SCALE, Math.min(newScale, MAX_SCALE));

		const boxCenterX = minX + boxWidth / 2;
		const boxCenterY = minY + boxHeight / 2;
		const newViewX = canvasWidth / 2 - boxCenterX * clampedScale;
		const newViewY = canvasHeight / 2 - boxCenterY * clampedScale;

		setView({ x: newViewX, y: newViewY });
		setScale(clampedScale);
	}, [layout.nodes]);

	// --- ADDED: The logic for the "Reset View" feature ---
	const handleResetView = useCallback(() => {
		setView({ x: 0, y: 0 });
		setScale(1);
	}, []);

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
		[interaction.mode]
	);

	const handleMouseMove = useCallback(
		(e: React.MouseEvent<HTMLDivElement>) => {
			setMousePosition({ x: e.clientX, y: e.clientY });
			if (interaction.mode === "panning") {
				const dx = e.clientX - interaction.start.x;
				const dy = e.clientY - interaction.start.y;
				setView((v) => ({ x: v.x + dx, y: v.y + dy }));
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
		},
		[interaction, scale, layout, onLayoutChange]
	);

	const handleMouseUp = useCallback(() => {
		if (interaction.mode === "panning" && canvasRef.current) {
			canvasRef.current.style.cursor = "grab";
		}
		if (interaction.mode !== "connecting") {
			setInteraction({ mode: "idle" });
		}
	}, [interaction]);

	const handleWheel = useCallback((e: WheelEvent) => {
		e.preventDefault();
		if (e.ctrlKey) {
			const zoomAmount = e.deltaY * -ZOOM_SENSITIVITY;
			setScale((prevScale) =>
				Math.min(Math.max(prevScale + zoomAmount, MIN_SCALE), MAX_SCALE)
			);
		} else {
			setView((v) => ({ x: v.x - e.deltaX, y: v.y - e.deltaY }));
		}
	}, []);

	useEffect(() => {
		const canvasElement = canvasRef.current;
		if (canvasElement) {
			canvasElement.addEventListener("wheel", handleWheel, { passive: false });
			return () => canvasElement.removeEventListener("wheel", handleWheel);
		}
	}, [handleWheel]);

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

			if (interaction.mode === "connecting" && interaction.fromNodeId) {
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

	return (
		<div
			ref={canvasRef}
			className="h-full w-full cursor-grab overflow-hidden bg-[var(--color-bg)]"
			style={{
				backgroundImage: `linear-gradient(var(--scrollbar-thumb) 1px, transparent 1px), linear-gradient(to right, var(--scrollbar-thumb) 1px, transparent 1px), linear-gradient(var(--color-bg-alt) 1px, transparent 1px), linear-gradient(to right, var(--color-bg-alt) 1px, transparent 1px)`,
				backgroundSize: `${100 * scale}px ${100 * scale}px, ${100 * scale}px ${100 * scale}px, ${20 * scale}px ${20 * scale}px, ${20 * scale}px ${20 * scale}px`,
				backgroundPosition: `${view.x}px ${view.y}px`,
			}}
			onMouseDown={handleMouseDown}
			onMouseMove={handleMouseMove}
			onMouseUp={handleMouseUp}
			onMouseLeave={handleMouseUp}
			onContextMenu={(e) => {
				e.preventDefault();
				if (interaction.mode === "connecting") setInteraction({ mode: "idle" });
			}}
		>
			<CanvasToolbar
				// --- MODIFIED: Pass the correct functions to the toolbar ---
				onResetView={handleResetView}
				onFitView={handleFitView}
				onToggleConnectorMode={handleToggleConnectorMode}
				isConnectorModeActive={interaction.mode === "connecting"}
			/>

			<motion.div
				className="absolute top-0 left-0"
				style={{ x: view.x, y: view.y, scale: scale }}
			>
				{layout.connections.map((conn) => {
					const start = getConnectionPoints(conn.fromNodeId, conn.fromHandle);
					const end = getConnectionPoints(conn.toNodeId, conn.toHandle);
					return (
						<CanvasConnector
							key={conn.id}
							x1={start.x}
							y1={start.y}
							x2={end.x}
							y2={end.y}
						/>
					);
				})}

				{layout.nodes.map((node) => {
					const props: NodeComponentProps = {
						node,
						onMouseDown: handleNodeMouseDown,
						onHandleClick: handleHandleClick,
						isConnecting: interaction.mode === "connecting",
					};
					if (node.type === "entry-point") {
						return (
							<DomainEntryPointCard
								key={node.id}
								{...props}
								domainName={selectedDomain}
							/>
						);
					}
					if (node.type === "rate-limit") {
						return <RateLimitCard key={node.id} {...props} />;
					}
					return null;
				})}

				<AnimatePresence>
					{interaction.mode === "connecting" && interaction.fromNodeId && (
						<CanvasConnector
							x1={interaction.fromPosition.x}
							y1={interaction.fromPosition.y}
							x2={mouseInCanvasCoords.x}
							y2={mouseInCanvasCoords.y}
						/>
					)}
				</AnimatePresence>
			</motion.div>
		</div>
	);
}
