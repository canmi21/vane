/* src/components/domain/rate-limit-card.tsx */

import { Zap } from "lucide-react";
import { motion } from "framer-motion";
import { type NodeComponentProps } from "./domain-entry-point-card";
import { CanvasNodeCard } from "./canvas-node-card";
import { type RateLimitNodeData, type CanvasNode } from "~/lib/canvas-layout";

interface RateLimitCardProps extends NodeComponentProps {
	node: CanvasNode<RateLimitNodeData>;
}

/**
 * A "middleware" node that displays rate limit info.
 * It uses the generic CanvasNodeCard for its structure and handles.
 */
export function RateLimitCard({
	node,
	onMouseDown,
	onMouseUp,
	onHandleClick,
	isConnecting,
	isSelected,
}: RateLimitCardProps) {
	return (
		// --- FINAL FIX: Add focus:outline-none and tabIndex={-1} to align with DomainEntryPointCard ---
		<motion.div
			className="absolute cursor-grab focus:outline-none"
			tabIndex={-1}
			style={{ x: node.x, y: node.y }}
			onMouseDown={(e) => {
				e.stopPropagation();
				onMouseDown(node.id, e);
			}}
			onMouseUp={() => onMouseUp(node.id)}
			whileTap={{ cursor: "grabbing" }}
		>
			<CanvasNodeCard
				icon={Zap}
				title="Rate Limit"
				inputs={node.inputs}
				outputs={node.outputs}
				isConnecting={isConnecting}
				isSelected={isSelected}
				onHandleClick={(handleId) => onHandleClick(node.id, handleId)}
			>
				<div className="text-center">
					<p className="text-2xl font-semibold text-[var(--color-text)]">
						{node.data.requests_per_second}
					</p>
					<p className="text-xs text-[var(--color-subtext)]">req/s</p>
				</div>
			</CanvasNodeCard>
		</motion.div>
	);
}
