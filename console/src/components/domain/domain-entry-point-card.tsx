/* src/components/domain/domain-entry-point-card.tsx */

import { Globe } from "lucide-react";
import * as Tooltip from "@radix-ui/react-tooltip";
import { motion } from "framer-motion";
import { type CanvasNode } from "~/lib/canvas-layout";
import React from "react";

export interface NodeComponentProps {
	node: CanvasNode;
	onMouseDown: (nodeId: string, e: React.MouseEvent) => void;
	onMouseUp: (nodeId: string) => void;
	onHandleClick: (nodeId: string, handleId: string) => void;
	isConnecting: boolean;
	isSelected: boolean;
}

interface DomainEntryPointCardProps extends NodeComponentProps {
	domainName: string;
}

export function DomainEntryPointCard({
	node,
	domainName,
	onMouseDown,
	onMouseUp,
	onHandleClick,
	isConnecting,
	isSelected,
}: DomainEntryPointCardProps) {
	const handleClicked = (e: React.MouseEvent) => {
		e.stopPropagation();
		onHandleClick(node.id, "output");
	};

	// --- FINAL FIX: Use the theme color for selection and a subtle ring effect ---
	const cardClasses = `relative w-64 rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-md transition-all duration-150 ${
		isSelected ? "ring-2 ring-[var(--color-theme-border)]" : ""
	}`;

	return (
		<motion.div
			className="absolute cursor-grab"
			style={{ x: node.x, y: node.y }}
			onMouseDown={(e) => onMouseDown(node.id, e)}
			onMouseUp={() => onMouseUp(node.id)}
			whileTap={{ cursor: "grabbing" }}
		>
			<Tooltip.Provider delayDuration={150}>
				<div className={cardClasses}>
					<div className="flex items-center gap-2 border-b border-[var(--color-bg-alt)] p-3">
						<Globe size={16} className="text-[var(--color-subtext)]" />
						<p className="text-xs font-semibold uppercase tracking-wider text-[var(--color-subtext)]">
							Entry Point
						</p>
					</div>
					<div className="p-2">
						<p className="truncate font-medium text-[var(--color-text)]">
							{domainName}
						</p>
					</div>
					<Tooltip.Root>
						<Tooltip.Trigger asChild>
							<div
								onClick={handleClicked}
								className={`absolute right-0 top-1/2 -translate-y-1/2 translate-x-1/2 h-3 w-3 rounded-full bg-[var(--color-bg)] border-2 border-[var(--color-theme-border)] transition-colors ${
									isConnecting
										? "cursor-crosshair hover:bg-[var(--color-theme-bg)]"
										: "cursor-pointer"
								}`}
							/>
						</Tooltip.Trigger>
						<Tooltip.Portal>
							<Tooltip.Content side="right" sideOffset={8}>
								<motion.div
									initial={{ opacity: 0, x: 5 }}
									animate={{ opacity: 1, x: 0 }}
									className="z-50 rounded-md bg-[var(--color-bg-alt)] px-2.5 py-1.5 text-xs font-medium text-[var(--color-text)] shadow-md"
								>
									Output
								</motion.div>
							</Tooltip.Content>
						</Tooltip.Portal>
					</Tooltip.Root>
				</div>
			</Tooltip.Provider>
		</motion.div>
	);
}
