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
import { PluginNodeCard } from "./plugin-node-card";

interface CanvasContentProps {
	layout: CanvasLayout;
	interaction: InteractionMode;
	selectedConnectionId: string | null;
	selectedNodeId: string | null;
	mouseInCanvasCoords: { x: number; y: number };
	selectedDomain: string;
	plugins: Plugin[];
	getConnectionPoints: (
		nodeId: string,
		handleId: string
	) => { x: number; y: number };
	handleNodeMouseDown: (nodeId: string, e: React.MouseEvent) => void;
	handleNodeMouseUp: (nodeId: string) => void;
	handleHandleClick: (nodeId: string, handleId: string) => void;
	handleConnectionClick: (connectionId: string) => void;
	// --- FINAL FIX: Add the new prop to the interface ---
	onUpdateNodeData: (nodeId: string, newData: Record<string, unknown>) => void;
}

/**
 * A purely presentational component that renders all the nodes and connections.
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
	onUpdateNodeData, // Destructure the prop
}: CanvasContentProps) {
	return (
		<>
			{/* ... (connection rendering is unchanged) ... */}
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

				return (
					<PluginNodeCard
						key={node.id}
						{...props}
						plugins={plugins}
						// --- FINAL FIX: Pass the prop down to the card ---
						onDataChange={onUpdateNodeData}
					/>
				);
			})}

			{/* ... (preview connector rendering is unchanged) ... */}
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
