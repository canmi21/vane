/* src/components/domain/canvas-node-card.tsx */

import React from "react";
import * as Tooltip from "@radix-ui/react-tooltip";
import { motion } from "framer-motion";
import { type NodeHandle } from "~/lib/canvas-layout";

// --- Type Definitions ---
interface CanvasNodeCardProps {
	icon: React.ElementType;
	title: string;
	inputs: NodeHandle[];
	outputs: NodeHandle[];
	children?: React.ReactNode;
	onHandleClick: (handleId: string) => void;
	isConnecting: boolean;
	isSelected: boolean;
}

/**
 * A generic card for "middleware" nodes, with height driven by the number of outputs.
 * The input handle is now always centered in the first "unit" of the body.
 */
export function CanvasNodeCard({
	icon: Icon,
	title,
	inputs,
	outputs,
	children,
	onHandleClick,
	isConnecting,
	isSelected,
}: CanvasNodeCardProps) {
	const headerRef = React.useRef<HTMLDivElement>(null);
	const [headerHeight, setHeaderHeight] = React.useState(41); // This is our "unit length"

	React.useLayoutEffect(() => {
		if (headerRef.current) {
			setHeaderHeight(headerRef.current.offsetHeight);
		}
	}, []);

	const bodyHeight = headerHeight * (outputs.length > 0 ? outputs.length : 1);
	const inputHandleY = headerHeight / 2;

	// --- FINAL FIX: Use the theme color for selection and a subtle ring effect ---
	const cardClasses = `relative w-64 rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-md transition-all duration-150 ${
		isSelected ? "ring-2 ring-[var(--color-theme-border)]" : ""
	}`;

	return (
		<Tooltip.Provider delayDuration={150}>
			<div className={cardClasses}>
				{/* Card Header */}
				<div
					ref={headerRef}
					className="flex items-center justify-between gap-2 border-b border-[var(--color-bg-alt)] p-3"
				>
					<div className="flex items-center gap-2">
						<Icon size={16} className="text-[var(--color-subtext)]" />
						<p className="text-xs font-semibold uppercase tracking-wider text-[var(--color-subtext)]">
							{title}
						</p>
					</div>
				</div>

				{/* Card Body */}
				<div className="relative" style={{ height: `${bodyHeight}px` }}>
					{/* Render Input Handles with the corrected positioning logic */}
					{inputs.map((handle) => (
						<Tooltip.Root key={handle.id}>
							<Tooltip.Trigger asChild>
								<div
									onClick={(e) => {
										e.stopPropagation();
										onHandleClick(handle.id);
									}}
									className={`absolute left-0 h-3 w-3 rounded-full bg-[var(--color-bg)] border-2 border-[var(--color-theme-border)] z-10 transition-colors ${
										isConnecting
											? "cursor-crosshair hover:bg-[var(--color-theme-bg)]"
											: "cursor-pointer"
									}`}
									style={{
										// Use the new, corrected Y position
										top: `${inputHandleY}px`,
										transform: "translate(-50%, -50%)",
									}}
								/>
							</Tooltip.Trigger>
							<Tooltip.Portal>
								<Tooltip.Content side="left" sideOffset={8}>
									<motion.div
										initial={{ opacity: 0, x: -5 }}
										animate={{ opacity: 1, x: 0 }}
										className="z-50 rounded-md bg-[var(--color-bg-alt)] px-2.5 py-1.5 text-xs font-medium text-[var(--color-text)] shadow-md"
									>
										{handle.label}
									</motion.div>
								</Tooltip.Content>
							</Tooltip.Portal>
						</Tooltip.Root>
					))}

					{/* Render Output Handles (evenly distributed) */}
					{outputs.map((handle, index) => {
						const positionPercent =
							outputs.length <= 1
								? 50
								: (100 / (outputs.length + 1)) * (index + 1);
						return (
							<Tooltip.Root key={handle.id}>
								<Tooltip.Trigger asChild>
									<div
										onClick={(e) => {
											e.stopPropagation();
											onHandleClick(handle.id);
										}}
										className={`absolute right-0 h-3 w-3 rounded-full bg-[var(--color-bg)] border-2 border-[var(--color-theme-border)] z-10 transition-colors ${
											isConnecting
												? "cursor-crosshair hover:bg-[var(--color-theme-bg)]"
												: "cursor-pointer"
										}`}
										style={{
											top: `${positionPercent}%`,
											transform: "translate(50%, -50%)",
										}}
									/>
								</Tooltip.Trigger>
								<Tooltip.Portal>
									<Tooltip.Content side="right" sideOffset={8}>
										<motion.div
											initial={{ opacity: 0, x: 5 }}
											animate={{ opacity: 1, x: 0 }}
											className="z-50 rounded-md bg-[var(--color-bg-alt)] px-2.5 py-1.5 text-xs font-medium text-[var(--color-text)] shadow-md"
										>
											{handle.label}
										</motion.div>
									</Tooltip.Content>
								</Tooltip.Portal>
							</Tooltip.Root>
						);
					})}

					<div className="p-3 h-full flex items-center justify-center">
						{children}
					</div>
				</div>
			</div>
		</Tooltip.Provider>
	);
}
