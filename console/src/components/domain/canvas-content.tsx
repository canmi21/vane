/* src/components/domain/canvas-content.tsx */

import { AnimatePresence } from "framer-motion";
import {
	type CanvasLayout,
	type CanvasNode,
	type EntryPointNodeData,
	type RateLimitNodeData,
} from "~/lib/canvas-layout";
import { type InteractionMode } from "~/hooks/use-canvas-interaction";
import {
	type NodeComponentProps,
	DomainEntryPointCard,
} from "./domain-entry-point-card";
import { RateLimitCard } from "./rate-limit-card";
import { CanvasConnector } from "./canvas-connector";
import React from "react";

interface CanvasContentProps {
	layout: CanvasLayout;
	interaction: InteractionMode;
	selectedConnectionId: string | null;
	selectedNodeId: string | null;
	mouseInCanvasCoords: { x: number; y: number };
	selectedDomain: string;
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
	getConnectionPoints,
	handleNodeMouseDown,
	handleNodeMouseUp,
	handleHandleClick,
	handleConnectionClick,
}: CanvasContentProps) {
	const PreviewConnector = CanvasConnector as React.FC<{
		x1: number;
		y1: number;
		x2: number;
		y2: number;
		id?: string;
		isSelected?: boolean;
		onClick?: (id: string) => void;
	}>;

	return (
		<>
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
					<PreviewConnector
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
