/* src/components/domain/plugin-node-card.tsx */

import { Zap, Puzzle } from "lucide-react";
import { motion } from "framer-motion";
import { type NodeComponentProps } from "./domain-entry-point-card";
import { CanvasNodeCard } from "./canvas-node-card";
import { type Plugin } from "~/hooks/use-plugin-data";
import React from "react";
import * as Switch from "@radix-ui/react-switch";

// --- Icon Mapping ---
const PLUGIN_ICONS: Record<string, React.ElementType> = {
	ratelimit: Zap,
};
const DefaultIcon = Puzzle;

// --- Component Props ---
interface PluginNodeCardProps extends NodeComponentProps {
	plugins: Plugin[];
	onDataChange: (nodeId: string, newData: Record<string, unknown>) => void;
}

/**
 * A dynamic component that renders any plugin-based node with editable fields.
 */
export function PluginNodeCard({
	node,
	plugins,
	onMouseDown,
	onMouseUp,
	onHandleClick,
	isConnecting,
	isSelected,
	onDataChange,
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
	const nodeData = node.data as Record<string, unknown>;

	const handleValueChange = (key: string, value: string | number | boolean) => {
		const newData = { ...nodeData, [key]: value };
		onDataChange(node.id, newData);
	};

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
				plugin={plugin}
				// --- FINAL FIX: Calculate and pass the number of input parameters ---
				inputParamCount={Object.keys(plugin.input_params).length}
			>
				<div className="w-full space-y-2 text-left">
					{Object.entries(plugin.input_params).map(([key, param]) => (
						<div key={key}>
							<label className="flex items-center justify-between text-xs text-[var(--color-subtext)] mb-1">
								{key.replace(/_/g, " ")}
								<span className="rounded bg-[var(--color-bg-alt)] px-1.5 py-0.5 font-mono text-xs">
									{param.type}
								</span>
							</label>
							{param.type === "string" && (
								<input
									type="text"
									value={(nodeData[key] as string) ?? ""}
									onChange={(e) => handleValueChange(key, e.target.value)}
									onMouseDown={(e) => e.stopPropagation()}
									className="w-full h-8 rounded-md border border-[var(--color-bg-alt)] bg-[var(--color-bg-alt)] px-2 text-sm text-[var(--color-text)] focus:outline-none focus:ring-1 focus:ring-[var(--color-theme-border)]"
								/>
							)}
							{param.type === "number" && (
								<input
									type="number"
									value={(nodeData[key] as number) ?? 0}
									onChange={(e) => {
										const num = parseFloat(e.target.value);
										handleValueChange(key, isNaN(num) ? 0 : num);
									}}
									onMouseDown={(e) => e.stopPropagation()}
									className="w-full h-8 rounded-md border border-[var(--color-bg-alt)] bg-[var(--color-bg-alt)] px-2 text-sm text-[var(--color-text)] focus:outline-none focus:ring-1 focus:ring-[var(--color-theme-border)]"
								/>
							)}
							{param.type === "boolean" && (
								<div className="flex items-center h-8">
									<Switch.Root
										checked={(nodeData[key] as boolean) ?? false}
										onCheckedChange={(checked) =>
											handleValueChange(key, checked)
										}
										onMouseDown={(e) => e.stopPropagation()}
										className="w-[36px] h-[20px] bg-[var(--color-bg-alt)] rounded-full relative data-[state=checked]:bg-[var(--color-theme-bg)] transition-colors"
									>
										<Switch.Thumb className="block w-[14px] h-[14px] bg-white rounded-full transition-transform duration-100 translate-x-1 data-[state=checked]:translate-x-[18px]" />
									</Switch.Root>
								</div>
							)}
						</div>
					))}
				</div>
			</CanvasNodeCard>
		</motion.div>
	);
}
