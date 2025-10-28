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
	onHandleClick,
	isConnecting,
}: RateLimitCardProps) {
	return (
		<motion.div
			className="absolute cursor-grab"
			style={{ x: node.x, y: node.y }}
			onMouseDown={(e) => onMouseDown(node.id, e)}
			whileTap={{ cursor: "grabbing" }}
		>
			{/* --- FIX: The Tooltip.Provider is now inside CanvasNodeCard, so it's removed from here. --- */}
			<CanvasNodeCard
				icon={Zap}
				title="Rate Limit"
				inputs={node.inputs}
				outputs={node.outputs}
				isConnecting={isConnecting}
				onHandleClick={(handleId) => onHandleClick(node.id, handleId)}
			>
				{/* Custom content for the body */}
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
