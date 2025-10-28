/* src/components/domain/domain-canvas.tsx */

import { useRef, useCallback } from "react";
import { motion, AnimatePresence } from "framer-motion";
import { CanvasToolbar } from "./canvas-toolbar";
import {
	type CanvasLayout,
	type CanvasNode,
	type EntryPointNodeData,
	type RateLimitNodeData,
} from "~/lib/canvas-layout";
import {
	DomainEntryPointCard,
	type NodeComponentProps,
} from "./domain-entry-point-card";
import { RateLimitCard } from "./rate-limit-card";
import { CanvasConnector } from "./canvas-connector";
import { useCanvasView } from "~/hooks/use-canvas-view";
import { useCanvasInteraction } from "~/hooks/use-canvas-interaction";

interface DomainCanvasProps {
	layout: CanvasLayout;
	onLayoutChange: (newLayout: CanvasLayout) => void;
	selectedDomain: string;
	onAddNode: (type: "rate-limit") => void;
}

export function DomainCanvas({
	layout,
	onLayoutChange,
	selectedDomain,
	onAddNode,
}: DomainCanvasProps) {
	const canvasRef = useRef<HTMLDivElement | null>(null);

	const { view, scale, panBy, handleFitView, handleResetView } = useCanvasView({
		canvasRef,
		nodes: layout.nodes,
	});

	const getConnectionPoints = useCallback(
		(nodeId: string, handleId: string): { x: number; y: number } => {
			const node = layout.nodes.find((n) => n.id === nodeId);
			if (!node) return { x: 0, y: 0 };

			const nodeWidth = 256;
			const headerHeight = 41;

			if (node.type === "entry-point") {
				const totalHeight = 83;
				return { x: node.x + nodeWidth, y: node.y + totalHeight / 2 };
			}

			if (node.type === "rate-limit") {
				const typedNode = node as CanvasNode<RateLimitNodeData>;
				const isInput = typedNode.inputs.some((h) => h.id === handleId);
				if (isInput) {
					// --- FINAL FIX: Calculate absolute Y by adding the body offset to the node's base Y. ---
					const bodyTopY = typedNode.y + headerHeight;
					const handleOffsetY = headerHeight / 2;
					return { x: typedNode.x, y: bodyTopY + handleOffsetY };
				} else {
					const bodyHeight =
						headerHeight *
						(typedNode.outputs.length > 0 ? typedNode.outputs.length : 1);
					const outputIndex = typedNode.outputs.findIndex(
						(h) => h.id === handleId
					);
					if (outputIndex === -1) return { x: typedNode.x, y: typedNode.y };

					const positionPercent =
						typedNode.outputs.length <= 1
							? 50
							: (100 / (typedNode.outputs.length + 1)) * (outputIndex + 1);
					const outputY =
						typedNode.y + headerHeight + bodyHeight * (positionPercent / 100);
					return { x: typedNode.x + nodeWidth, y: outputY };
				}
			}

			return { x: node.x, y: node.y };
		},
		[layout.nodes]
	);

	const {
		interaction,
		mouseInCanvasCoords,
		handleMouseDown,
		handleMouseMove,
		handleMouseUp,
		handleNodeMouseDown,
		handleHandleClick,
		handleToggleConnectorMode,
	} = useCanvasInteraction({
		scale,
		view,
		layout,
		onLayoutChange,
		panBy,
		getConnectionPoints,
		canvasRef,
	});

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
				if (interaction.mode === "connecting") handleToggleConnectorMode();
			}}
		>
			<CanvasToolbar
				onResetView={handleResetView}
				onFitView={handleFitView}
				onToggleConnectorMode={handleToggleConnectorMode}
				isConnectorModeActive={interaction.mode === "connecting"}
				onAddNode={onAddNode}
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
								node={node as CanvasNode<EntryPointNodeData>}
								domainName={selectedDomain}
							/>
						);
					}
					if (node.type === "rate-limit") {
						return (
							<RateLimitCard
								key={node.id}
								{...props}
								node={node as CanvasNode<RateLimitNodeData>}
							/>
						);
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
