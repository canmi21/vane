/* src/components/domain/plugin-node-card.tsx */

import { Zap, Puzzle } from "lucide-react";
import { motion } from "framer-motion";
import { type NodeComponentProps } from "./domain-entry-point-card";
import { CanvasNodeCard } from "./canvas-node-card";
import { type Plugin } from "~/hooks/use-plugin-data";
import React from "react";

// --- Icon Mapping ---
const PLUGIN_ICONS: Record<string, React.ElementType> = {
	ratelimit: Zap,
};
const DefaultIcon = Puzzle;

// --- Component Props ---
interface PluginNodeCardProps extends NodeComponentProps {
	plugins: Plugin[];
}

/**
 * A dynamic component that renders any plugin-based node.
 */
export function PluginNodeCard({
	node,
	plugins,
	onMouseDown,
	onMouseUp,
	onHandleClick,
	isConnecting,
	isSelected,
}: PluginNodeCardProps) {
	const plugin = plugins.find((p) => p.name === node.type);

	if (!plugin) {
		return (
			<motion.div style={{ x: node.x, y: node.y }} className="absolute">
				<div className="w-64 rounded-lg border border-red-500/50 bg-red-500/10 p-4 text-center">
					<p className="font-bold text-red-400">Unknown Plugin</p>
					<p className="text-xs text-red-400/80">
						Type: "{node.type}" not found.
					</p>
				</div>
			</motion.div>
		);
	}

	const Icon = PLUGIN_ICONS[plugin.name] ?? DefaultIcon;
	const title = plugin.name.replace(/-/g, " ");

	return (
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
				icon={Icon}
				title={title}
				inputs={node.inputs}
				outputs={node.outputs}
				isConnecting={isConnecting}
				isSelected={isSelected}
				onHandleClick={(handleId) => onHandleClick(node.id, handleId)}
				// --- FINAL FIX: Pass the entire plugin object down ---
				plugin={plugin}
			>
				{/* The body of the card still displays the current values */}
				<div className="text-center">
					{Object.keys(plugin.input_params).map((key) => (
						<div key={key}>
							<p className="text-2xl font-semibold text-[var(--color-text)]">
								{(node.data as Record<string, unknown>)[key]?.toString() ??
									"N/A"}
							</p>
							<p className="text-xs text-[var(--color-subtext)]">
								{key.replace(/_/g, " ")}
							</p>
						</div>
					))}
				</div>
			</CanvasNodeCard>
		</motion.div>
	);
}
