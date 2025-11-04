/* src/components/domain/error-page-node-card.tsx */

import { FileWarning } from "lucide-react";
import { motion } from "framer-motion";
import { type NodeComponentProps } from "./domain-entry-point-card";
import { CanvasNodeCard } from "./canvas-node-card";
import { type ErrorPageNodeData } from "~/lib/canvas-layout";

// --- Component Props ---
interface ErrorPageNodeCardProps extends NodeComponentProps {
	onDataChange: (nodeId: string, newData: Record<string, unknown>) => void;
}

// --- Hardcoded field definitions for the error page form ---
const errorPageFields: {
	key: keyof ErrorPageNodeData;
	label: string;
	type: "number" | "string";
}[] = [
	{ key: "status_code", label: "Status Code", type: "number" },
	{ key: "status_description", label: "Status Description", type: "string" },
	{ key: "reason", label: "Reason", type: "string" },
	{ key: "request_id", label: "Request ID", type: "string" },
	{ key: "timestamp", label: "Timestamp", type: "string" },
	{ key: "version", label: "Version", type: "string" },
	{ key: "request_ip", label: "Request IP", type: "string" },
	{ key: "visitor_tip", label: "Visitor Tip", type: "string" },
	{ key: "admin_guide", label: "Admin Guide", type: "string" },
];

/**
 * A dedicated component for the 'Return Error Page' node.
 */
export function ErrorPageNodeCard({
	node,
	onMouseDown,
	onMouseUp,
	onHandleClick,
	isConnecting,
	isSelected,
	onDataChange,
}: ErrorPageNodeCardProps) {
	const nodeData = node.data as ErrorPageNodeData;

	const handleValueChange = (
		key: keyof ErrorPageNodeData,
		value: string | number
	) => {
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
				icon={FileWarning}
				title="Return Error Page"
				inputs={node.inputs}
				outputs={node.outputs}
				isConnecting={isConnecting}
				isSelected={isSelected}
				onHandleClick={(handleId) => onHandleClick(node.id, handleId)}
				inputParamCount={errorPageFields.length}
			>
				<div className="w-full space-y-2 text-left">
					{errorPageFields.map(({ key, label, type }) => (
						<div key={key}>
							<label className="flex items-center justify-between text-xs text-[var(--color-subtext)] mb-1 capitalize">
								{label}
							</label>
							<input
								type={type}
								value={nodeData[key] ?? (type === "number" ? 0 : "")}
								onChange={(e) => {
									const value =
										type === "number"
											? parseFloat(e.target.value) || 0
											: e.target.value;
									handleValueChange(key, value);
								}}
								onMouseDown={(e) => e.stopPropagation()}
								className="w-full h-8 rounded-md border border-[var(--color-bg-alt)] bg-[var(--color-bg-alt)] px-2 text-sm text-[var(--color-text)] focus:outline-none focus:ring-1 focus:ring-[var(--color-theme-border)]"
							/>
						</div>
					))}
				</div>
			</CanvasNodeCard>
		</motion.div>
	);
}
