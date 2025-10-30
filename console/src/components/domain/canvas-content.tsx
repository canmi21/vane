/* src/components/domain/canvas-content.tsx */

import { AnimatePresence } from "framer-motion";
import {
	type CanvasLayout,
	type CanvasNode,
	type EntryPointNodeData,
} from "~/lib/canvas-layout";
import { type InteractionMode } from "~/hooks/use-canvas-interaction";
import {
	type NodeComponentProps,
	DomainEntryPointCard,
} from "./domain-entry-point-card";
import { CanvasConnector } from "./canvas-connector";
import React from "react";
import { type Plugin } from "~/hooks/use-plugin-data";
import { PluginNodeCard } from "./plugin-node-card"; // Import the new generic card

interface CanvasContentProps {
	layout: CanvasLayout;
	interaction: InteractionMode;
	selectedConnectionId: string | null;
	selectedNodeId: string | null;
	mouseInCanvasCoords: { x: number; y: number };
	selectedDomain: string;
	plugins: Plugin[]; // Pass plugins down
	getConnectionPoints: (
		nodeId: string,
		handleId: string
	) => { x: number; y: number };
	handleNodeMouseDown: (nodeId: string, e: React.MouseEvent) => void;
	handleNodeMouseUp: (nodeId: string) => void;
	handleHandleClick: (nodeId: string, handleId: string) => void;
	handleConnectionClick: (connectionId: string) => void;
}

/**
 * A purely presentational component that renders all the nodes and connections
 * inside the canvas viewport.
 */
export function CanvasContent({
	layout,
	interaction,
	selectedConnectionId,
	selectedNodeId,
	mouseInCanvasCoords,
	selectedDomain,
	plugins,
	getConnectionPoints,
	handleNodeMouseDown,
	handleNodeMouseUp,
	handleHandleClick,
	handleConnectionClick,
}: CanvasContentProps) {
	return (
		<>
			{/* Connection rendering remains the same */}
			{layout.connections.map((conn) => {
				const start = getConnectionPoints(conn.fromNodeId, conn.fromHandle);
				const end = getConnectionPoints(conn.toNodeId, conn.toHandle);
				return (
					<CanvasConnector
						key={conn.id}
						id={conn.id}
						x1={start.x}
						y1={start.y}
						x2={end.x}
						y2={end.y}
						isSelected={conn.id === selectedConnectionId}
						onClick={handleConnectionClick}
					/>
				);
			})}

			{/* Node rendering is now dynamic */}
			{layout.nodes.map((node) => {
				const props: NodeComponentProps = {
					node,
					onMouseDown: handleNodeMouseDown,
					onMouseUp: handleNodeMouseUp,
					onHandleClick: handleHandleClick,
					isConnecting: interaction.mode === "connecting",
					isSelected: node.id === selectedNodeId,
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

				// --- FINAL FIX: All other nodes are rendered by the generic PluginNodeCard ---
				return <PluginNodeCard key={node.id} {...props} plugins={plugins} />;
			})}

			{/* Preview connector rendering remains the same */}
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
		</>
	);
}
