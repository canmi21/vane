/* src/components/domain/rate-limit-card.tsx */

import { Zap } from "lucide-react";
import * as Tooltip from "@radix-ui/react-tooltip";
import { motion } from "framer-motion";
import { type NodeComponentProps } from "./domain-entry-point-card"; // Reuse the interface

export function RateLimitCard({
	node,
	onMouseDown,
	onHandleClick,
	isConnecting,
}: NodeComponentProps) {
	return (
		<motion.div
			className="absolute cursor-grab"
			style={{ x: node.x, y: node.y }}
			onMouseDown={(e) => onMouseDown(node.id, e)}
			whileTap={{ cursor: "grabbing" }}
		>
			<Tooltip.Provider delayDuration={150}>
				<div className="relative w-64 rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-md">
					{/* Input Handle */}
					<Tooltip.Root>
						<Tooltip.Trigger asChild>
							<div
								data-handle-id="input"
								onClick={(e) => onHandleClick(node.id, "input", e)}
								className={`absolute left-0 top-1/2 -translate-y-1/2 -translate-x-1/2 h-3 w-3 rounded-full bg-[var(--color-bg)] border-2 border-[var(--color-theme-border)] z-10 transition-colors ${
									isConnecting
										? "cursor-crosshair hover:bg-[var(--color-theme-bg)]"
										: "cursor-pointer"
								}`}
							/>
						</Tooltip.Trigger>
						<Tooltip.Portal>
							<Tooltip.Content side="left" sideOffset={8}>
								<motion.div
									initial={{ opacity: 0, x: -5 }}
									animate={{ opacity: 1, x: 0 }}
									className="z-50 rounded-md bg-[var(--color-bg-alt)] px-2.5 py-1.5 text-xs font-medium text-[var(--color-text)] shadow-md"
								>
									Input
								</motion.div>
							</Tooltip.Content>
						</Tooltip.Portal>
					</Tooltip.Root>

					{/* Header */}
					<div className="flex items-center justify-between gap-2 border-b border-[var(--color-bg-alt)] p-3">
						<div className="flex items-center gap-2">
							<Zap size={16} className="text-[var(--color-subtext)]" />
							<p className="text-xs font-semibold uppercase tracking-wider text-[var(--color-subtext)]">
								Rate Limit
							</p>
						</div>
					</div>

					{/* Body */}
					<div className="h-20" />
				</div>
			</Tooltip.Provider>
		</motion.div>
	);
}
