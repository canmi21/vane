/* src/components/domain/canvas-node-card.tsx */

import React from "react";
import * as Tooltip from "@radix-ui/react-tooltip";
import { motion } from "framer-motion";
import { type NodeHandle } from "~/lib/canvas-layout";
import { type Plugin } from "~/hooks/use-plugin-data";
import { InfoTooltip } from "./info-tooltip"; // Import the new reusable component

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
	// --- FINAL FIX: This now only needs the full plugin object ---
	plugin?: Plugin;
}

/**
 * A generic card for "middleware" nodes, with height driven by the number of outputs.
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
	plugin,
}: CanvasNodeCardProps) {
	const headerRef = React.useRef<HTMLDivElement>(null);
	const [headerHeight, setHeaderHeight] = React.useState(41);

	React.useLayoutEffect(() => {
		if (headerRef.current) {
			setHeaderHeight(headerRef.current.offsetHeight);
		}
	}, []);

	const bodyHeight = headerHeight * (outputs.length > 0 ? outputs.length : 1);
	const inputHandleY = headerHeight / 2;

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
					<div className="flex items-center gap-2 overflow-hidden">
						<Icon size={16} className="text-[var(--color-subtext)]" />
						<p className="truncate text-xs font-semibold uppercase tracking-wider text-[var(--color-subtext)]">
							{title}
						</p>
					</div>

					{/* --- FINAL FIX: Use the reusable InfoTooltip component --- */}
					{plugin && <InfoTooltip plugin={plugin} />}
				</div>

				{/* Card Body (unchanged) */}
				<div className="relative" style={{ height: `${bodyHeight}px` }}>
					{inputs.map((handle) => (
						<HandleTooltip
							key={handle.id}
							handle={handle}
							side="left"
							onClick={onHandleClick}
							isConnecting={isConnecting}
							style={{
								top: `${inputHandleY}px`,
								transform: "translate(-50%, -50%)",
							}}
						/>
					))}
					{outputs.map((handle, index) => {
						const positionPercent =
							outputs.length <= 1
								? 50
								: (100 / (outputs.length + 1)) * (index + 1);
						return (
							<HandleTooltip
								key={handle.id}
								handle={handle}
								side="right"
								onClick={onHandleClick}
								isConnecting={isConnecting}
								style={{
									top: `${positionPercent}%`,
									transform: "translate(50%, -50%)",
									right: 0,
								}}
							/>
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

// --- Sub-components for cleaner rendering ---

interface HandleTooltipProps {
	handle: NodeHandle;
	side: "left" | "right";
	onClick: (handleId: string) => void;
	isConnecting: boolean;
	style: React.CSSProperties;
}

const HandleTooltip = ({
	handle,
	side,
	onClick,
	isConnecting,
	style,
}: HandleTooltipProps) => (
	<Tooltip.Root>
		<Tooltip.Trigger asChild>
			<div
				onClick={(e) => {
					e.stopPropagation();
					onClick(handle.id);
				}}
				className={`absolute h-3 w-3 rounded-full bg-[var(--color-bg)] border-2 border-[var(--color-theme-border)] z-10 transition-colors ${
					isConnecting
						? "cursor-crosshair hover:bg-[var(--color-theme-bg)]"
						: "cursor-pointer"
				}`}
				style={style}
			/>
		</Tooltip.Trigger>
		<Tooltip.Portal>
			<Tooltip.Content side={side} sideOffset={8}>
				<motion.div
					initial={{ opacity: 0, x: side === "left" ? -5 : 5 }}
					animate={{ opacity: 1, x: 0 }}
					className="z-50 rounded-md bg-[var(--color-bg-alt)] px-2.5 py-1.5 text-xs font-medium text-[var(--color-text)] shadow-md"
				>
					{handle.label}
				</motion.div>
			</Tooltip.Content>
		</Tooltip.Portal>
	</Tooltip.Root>
);
