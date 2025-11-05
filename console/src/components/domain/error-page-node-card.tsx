/* src/components/domain/error-page-node-card.tsx */

import { FileWarning } from "lucide-react";
import { motion } from "framer-motion";
import { type NodeComponentProps } from "./domain-entry-point-card";
import { CanvasNodeCard } from "./canvas-node-card";
import { type ErrorPageNodeData } from "~/lib/canvas-layout";
import { useState, useEffect } from "react";

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
							{/* --- FINAL FIX: Removed the type indicator from the label. --- */}
							<label className="flex items-center justify-between text-xs text-[var(--color-subtext)] mb-1 capitalize">
								{label}
							</label>
							<EditableInput
								type={type}
								initialValue={nodeData[key]}
								onCommit={(newValue) => {
									handleValueChange(key, newValue as string | number);
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
	type: "string" | "number";
	initialValue: unknown;
	onCommit: (newValue: string | number) => void;
}

function EditableInput({ type, initialValue, onCommit }: EditableInputProps) {
	const [localValue, setLocalValue] = useState(String(initialValue ?? ""));

	useEffect(() => {
		setLocalValue(String(initialValue ?? ""));
	}, [initialValue]);

	const handleBlur = () => {
		const value = localValue.trim();

		if (value.startsWith("{{") && value.endsWith("}}")) {
			onCommit(value);
			return;
		}

		if (value === "") {
			if (type === "number") onCommit(0);
			else onCommit("");
			return;
		}

		switch (type) {
			case "number": {
				const num = parseFloat(value);
				if (!isNaN(num)) {
					onCommit(num);
				} else {
					setLocalValue(String(initialValue));
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
			type="text"
			value={localValue}
			onChange={(e) => setLocalValue(e.target.value)}
			onBlur={handleBlur}
			onMouseDown={(e) => e.stopPropagation()}
			placeholder={type}
			className="w-full h-8 rounded-md border border-[var(--color-bg-alt)] bg-[var(--color-bg-alt)] px-2 text-sm text-[var(--color-text)] focus:outline-none focus:ring-1 focus:ring-[var(--color-theme-border)] placeholder:text-[var(--color-subtext)]/50"
		/>
	);
}
