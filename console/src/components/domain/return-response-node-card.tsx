/* src/components/domain/return-response-node-card.tsx */

import { Send } from "lucide-react";
import { motion } from "framer-motion";
import { type NodeComponentProps } from "./domain-entry-point-card";
import { CanvasNodeCard } from "./canvas-node-card";
import { type ReturnResponseNodeData } from "~/lib/canvas-layout";
import React, { useState, useEffect } from "react";

// --- Component Props ---
interface ReturnResponseNodeCardProps extends NodeComponentProps {
	onDataChange: (nodeId: string, newData: Record<string, unknown>) => void;
}

// --- Hardcoded field definitions for the response form ---
// --- FINAL FIX: Changed header and body types from 'textarea' to 'string' for single-line inputs. ---
const responseFields: {
	key: keyof ReturnResponseNodeData;
	label: string;
	type: "number" | "string";
}[] = [
	{ key: "status_code", label: "Status Code", type: "number" },
	{ key: "header", label: "Header", type: "string" },
	{ key: "body", label: "Body", type: "string" },
];

/**
 * A dedicated component for the 'Return Response' node.
 */
export function ReturnResponseNodeCard({
	node,
	onMouseDown,
	onMouseUp,
	onHandleClick,
	isConnecting,
	isSelected,
	onDataChange,
}: ReturnResponseNodeCardProps) {
	const nodeData = node.data as ReturnResponseNodeData;

	const handleValueChange = (
		key: keyof ReturnResponseNodeData,
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
				icon={Send}
				title="Return Response"
				inputs={node.inputs}
				outputs={node.outputs}
				isConnecting={isConnecting}
				isSelected={isSelected}
				onHandleClick={(handleId) => onHandleClick(node.id, handleId)}
				inputParamCount={responseFields.length}
			>
				<div className="w-full space-y-2 text-left">
					{responseFields.map(({ key, label, type }) => (
						<div key={key}>
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

// --- Sub-component for handling validation logic ---

// --- FINAL FIX: Removed 'textarea' from the type definition. ---
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

	const handleBlur = (e: React.FocusEvent<HTMLInputElement>) => {
		const value = e.target.value.trim();

		if (value.startsWith("{{") && value.endsWith("}}")) {
			onCommit(value);
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

	// --- FINAL FIX: This component now only renders a single-line input. ---
	return (
		<input
			type="text"
			value={localValue}
			onChange={(e) => setLocalValue(e.target.value)}
			onBlur={handleBlur}
			onMouseDown={(e) => e.stopPropagation()}
			className="w-full h-8 rounded-md border border-[var(--color-bg-alt)] bg-[var(--color-bg-alt)] px-2 text-sm text-[var(--color-text)] focus:outline-none focus:ring-1 focus:ring-[var(--color-theme-border)]"
		/>
	);
}
