/* src/components/domain/plugin-node-card.tsx */

import { Zap, Puzzle } from "lucide-react";
import { motion } from "framer-motion";
import { type NodeComponentProps } from "./domain-entry-point-card";
import { CanvasNodeCard } from "./canvas-node-card";
import { type Plugin } from "~/hooks/use-plugin-data";
import React, { useState, useEffect } from "react";

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
				inputParamCount={Object.keys(plugin.input_params).length}
			>
				<div className="w-full space-y-2 text-left">
					{Object.entries(plugin.input_params).map(([key, param]) => (
						<div key={key}>
							<label className="flex items-center justify-between text-xs text-[var(--color-subtext)] mb-1 capitalize">
								{key.replace(/_/g, " ")}
								<span className="rounded bg-[var(--color-bg-alt)] px-1.5 py-0.5 font-mono text-xs">
									{param.type}
								</span>
							</label>
							{/* --- FINAL FIX: Use the new EditableInput sub-component for all types --- */}
							<EditableInput
								type={param.type as "string" | "number" | "boolean"}
								initialValue={nodeData[key]}
								onCommit={(newValue) => {
									handleValueChange(key, newValue);
								}}
							/>
						</div>
					))}
				</div>
			</CanvasNodeCard>
		</motion.div>
	);
}

// --- Sub-component for handling the new validation logic ---

interface EditableInputProps {
	type: "string" | "number" | "boolean";
	initialValue: unknown;
	onCommit: (newValue: string | number | boolean) => void;
}

/**
 * An input that holds a local string state for editing and validates/commits on blur.
 */
function EditableInput({ type, initialValue, onCommit }: EditableInputProps) {
	const [localValue, setLocalValue] = useState(String(initialValue ?? ""));

	// Sync local state if the initial (prop) value changes from the outside.
	useEffect(() => {
		setLocalValue(String(initialValue ?? ""));
	}, [initialValue]);

	const handleBlur = () => {
		const value = localValue.trim();

		// Rule: If the value is a template variable, treat it as a valid string and commit.
		if (value.startsWith("{{") && value.endsWith("}}")) {
			onCommit(value);
			return;
		}

		// Perform type-specific validation.
		switch (type) {
			case "number": {
				const num = parseFloat(value);
				if (!isNaN(num)) {
					onCommit(num);
				} else {
					setLocalValue(String(initialValue)); // Revert on invalid
				}
				break;
			}
			case "boolean": {
				const lowerValue = value.toLowerCase();
				if (lowerValue === "true" || lowerValue === "false") {
					onCommit(lowerValue === "true");
				} else {
					setLocalValue(String(initialValue)); // Revert on invalid
				}
				break;
			}
			case "string":
			default:
				onCommit(value);
				break;
		}
	};

	return (
		<input
			type="text" // Always "text" to allow free editing.
			value={localValue}
			onChange={(e) => setLocalValue(e.target.value)}
			onBlur={handleBlur}
			onMouseDown={(e) => e.stopPropagation()}
			className="w-full h-8 rounded-md border border-[var(--color-bg-alt)] bg-[var(--color-bg-alt)] px-2 text-sm text-[var(--color-text)] focus:outline-none focus:ring-1 focus:ring-[var(--color-theme-border)]"
		/>
	);
}
