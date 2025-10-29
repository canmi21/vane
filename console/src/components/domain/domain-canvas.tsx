/* src/components/domain/domain-canvas.tsx */

import { useRef } from "react";
import { motion } from "framer-motion";
import { CanvasToolbar } from "./canvas-toolbar";
import { type CanvasLayout } from "~/lib/canvas-layout";
import { useCanvasView } from "~/hooks/use-canvas-view";
import { useCanvasInteraction } from "~/hooks/use-canvas-interaction";
import { useConnectionPoints } from "~/hooks/use-connection-points";
import { CanvasContent } from "./canvas-content";

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

	const { getConnectionPoints } = useConnectionPoints(layout);

	const {
		interaction,
		mouseInCanvasCoords,
		selectedConnectionId,
		handleMouseDown,
		handleMouseMove,
		handleMouseUp,
		handleNodeMouseDown,
		handleHandleClick,
		handleConnectionClick,
		handleDeleteSelectedConnection,
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
				selectedConnectionId={selectedConnectionId}
				onDeleteSelectedConnection={handleDeleteSelectedConnection}
			/>

			<motion.div
				className="absolute top-0 left-0"
				style={{ x: view.x, y: view.y, scale: scale }}
			>
				<CanvasContent
					layout={layout}
					interaction={interaction}
					selectedConnectionId={selectedConnectionId}
					mouseInCanvasCoords={mouseInCanvasCoords}
					selectedDomain={selectedDomain}
					getConnectionPoints={getConnectionPoints}
					handleNodeMouseDown={handleNodeMouseDown}
					handleHandleClick={handleHandleClick}
					handleConnectionClick={handleConnectionClick}
				/>
			</motion.div>
		</div>
	);
}
