/* src/components/domain/domain-canvas.tsx */

import { useRef } from "react";
import { motion } from "framer-motion";
import { CanvasToolbar } from "./canvas-toolbar";
import { type CanvasLayout } from "~/lib/canvas-layout";
import { useCanvasView } from "~/hooks/use-canvas-view";
import { useCanvasInteraction } from "~/hooks/use-canvas-interaction";
import { useConnectionPoints } from "~/hooks/use-connection-points";
import { CanvasContent } from "./canvas-content";
import { type Plugin } from "~/hooks/use-plugin-data"; // Import type

interface DomainCanvasProps {
	layout: CanvasLayout;
	onLayoutChange: (newLayout: CanvasLayout) => void;
	selectedDomain: string;
	plugins: Plugin[]; // Accept plugins
	onAddNode: (plugin: Plugin) => void; // Update signature
}

export function DomainCanvas({
	layout,
	onLayoutChange,
	selectedDomain,
	plugins,
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
		selectedNodeId,
		handleMouseDown,
		handleMouseMove,
		handleMouseUp,
		handleNodeMouseDown,
		handleHandleClick,
		handleConnectionClick,
		handleDeleteSelectedConnection,
		handleDeleteSelectedNode,
		handleToggleConnectorMode,
		handleContextMenu,
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
			onMouseUp={() => handleMouseUp()}
			onMouseLeave={() => handleMouseUp()}
			onContextMenu={handleContextMenu}
		>
			<CanvasToolbar
				plugins={plugins} // Pass to toolbar
				onResetView={handleResetView}
				onFitView={handleFitView}
				onToggleConnectorMode={handleToggleConnectorMode}
				isConnectorModeActive={interaction.mode === "connecting"}
				onAddNode={onAddNode}
				selectedConnectionId={selectedConnectionId}
				onDeleteSelectedConnection={handleDeleteSelectedConnection}
				selectedNodeId={selectedNodeId}
				onDeleteSelectedNode={handleDeleteSelectedNode}
			/>

			<motion.div
				className="absolute top-0 left-0"
				style={{ x: view.x, y: view.y, scale: scale }}
			>
				<CanvasContent
					layout={layout}
					interaction={interaction}
					selectedConnectionId={selectedConnectionId}
					selectedNodeId={selectedNodeId}
					mouseInCanvasCoords={mouseInCanvasCoords}
					selectedDomain={selectedDomain}
					plugins={plugins} // Pass to content
					getConnectionPoints={getConnectionPoints}
					handleNodeMouseDown={handleNodeMouseDown}
					handleNodeMouseUp={handleMouseUp}
					handleHandleClick={handleHandleClick}
					handleConnectionClick={handleConnectionClick}
				/>
			</motion.div>
		</div>
	);
}
