/* src/components/domain/domain-canvas.tsx */

import React, { useState, useRef, useCallback, useEffect } from "react";
import { motion, AnimatePresence } from "framer-motion";
import { CanvasToolbar } from "./canvas-toolbar";
import { type CanvasLayout, type CanvasConnection } from "~/lib/canvas-layout";
import { DomainEntryPointCard } from "./domain-entry-point-card";
import { RateLimitCard } from "./rate-limit-card";
import { CanvasConnector } from "./canvas-connector";
import { nanoid } from "nanoid";

// --- Constants & Types ---
const ZOOM_SENSITIVITY = 0.001;
const MIN_SCALE = 0.2;
const MAX_SCALE = 3;

type InteractionMode =
	| { mode: "idle" }
	| { mode: "panning"; start: { x: number; y: number } }
	| { mode: "dragging"; nodeId: string; start: { x: number; y: number } }
	| {
			mode: "connecting";
			fromNodeId: string;
			fromHandle: string;
			fromPosition: { x: number; y: number };
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
			const entryPointHeight = 73;
			const rateLimitHeight = 125;

			if (node.type === "entry-point") {
				return { x: node.x + nodeWidth, y: node.y + entryPointHeight / 2 };
			}
			if (node.type === "rate-limit" && handleId === "input") {
				return { x: node.x, y: node.y + rateLimitHeight / 2 };
			}
			return { x: node.x, y: node.y };
		},
		[layout.nodes]
	);

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

	// --- FIX: Re-added the manual event listener for the wheel event ---
	useEffect(() => {
		const canvasElement = canvasRef.current;
		if (canvasElement) {
			// This { passive: false } option is crucial to prevent the console warning.
			canvasElement.addEventListener("wheel", handleWheel, { passive: false });
			return () => {
				canvasElement.removeEventListener("wheel", handleWheel);
			};
		}
	}, [handleWheel]);

	const handleNodeMouseDown = useCallback(
		(nodeId: string, e: React.MouseEvent) => {
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
		(nodeId: string, handleId: string, e: React.MouseEvent) => {
			e.stopPropagation();
			if (interaction.mode !== "connecting") return;

			const handlePos = getConnectionPoints(nodeId, handleId);

			if (!interaction.fromNodeId) {
				setInteraction({
					mode: "connecting",
					fromNodeId: nodeId,
					fromHandle: handleId,
					fromPosition: handlePos,
				});
				return;
			}
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
			// onWheel is now correctly handled by the useEffect hook above
			onContextMenu={(e) => {
				e.preventDefault();
				if (interaction.mode === "connecting") {
					setInteraction({ mode: "idle" });
				}
			}}
		>
			<CanvasToolbar
				onResetView={() => {
					setView({ x: 0, y: 0 });
					setScale(1);
				}}
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
					const props = {
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
			</motion.div>

			<AnimatePresence>
				{interaction.mode === "connecting" && interaction.fromNodeId && (
					<svg className="absolute top-0 left-0 w-full h-full pointer-events-none z-20">
						<CanvasConnector
							x1={interaction.fromPosition.x * scale + view.x}
							y1={interaction.fromPosition.y * scale + view.y}
							x2={mousePosition.x}
							y2={mousePosition.y}
						/>
					</svg>
				)}
			</AnimatePresence>
		</div>
	);
}
