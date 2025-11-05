/* src/components/domain/canvas-node-card.tsx */

import React from "react";
import * as Tooltip from "@radix-ui/react-tooltip";
import { motion } from "framer-motion";
import { type NodeHandle } from "~/lib/canvas-layout";
import { type Plugin } from "~/hooks/use-plugin-data";
import { InfoTooltip } from "./info-tooltip";

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
	plugin?: Plugin;
	inputParamCount?: number;
}

/**
 * A generic card for "middleware" nodes with fully dynamic height and handle positioning.
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
	inputParamCount = 0,
}: CanvasNodeCardProps) {
	const headerRef = React.useRef<HTMLDivElement>(null);
	const [headerHeight, setHeaderHeight] = React.useState(41);

	React.useLayoutEffect(() => {
		if (headerRef.current) {
			setHeaderHeight(headerRef.current.offsetHeight);
		}
	}, []);

	// --- FINAL FIX: A new, structurally accurate height calculation formula. ---
	// This formula precisely models the actual layout: padding + rows + gaps between rows.
	// It replaces all previous linear estimations which were causing clipping and spacing issues.
	const heightFromOutputs =
		headerHeight * (outputs.length > 0 ? outputs.length : 1);

	const ROW_HEIGHT = 52; // Actual height of one item: <label>(~16) + mb-1(4) + <input h-8>(32)
	const GAP_HEIGHT = 8; // From the `space-y-2` class
	const PADDING_VERTICAL = 24; // From the `p-3` class (12px top + 12px bottom)

	// The total height is the sum of all rows, the gaps between them, and the vertical padding.
	const heightFromInputs =
		inputParamCount > 0
			? inputParamCount * ROW_HEIGHT +
				(inputParamCount - 1) * GAP_HEIGHT +
				PADDING_VERTICAL
			: PADDING_VERTICAL; // If there are no inputs, the height is just the padding.

	const bodyHeight = Math.max(heightFromOutputs, heightFromInputs);

	// This calculation for handle positions is now based on a consistent and accurate height.
	const firstOutputPositionPercent =
		outputs.length <= 1 ? 50 : 100 / (outputs.length + 1);
	const inputHandleY =
		outputs.length > 0
			? bodyHeight * (firstOutputPositionPercent / 100)
			: headerHeight / 2;

	const cardClasses = `relative w-64 rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-md transition-all duration-150 ${
		isSelected ? "ring-2 ring-[var(--color-theme-border)]" : ""
	}`;

	return (
		<Tooltip.Provider delayDuration={150}>
			<div className={cardClasses}>
				{/* Card Header (unchanged) */}
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
					{plugin && <InfoTooltip plugin={plugin} />}
				</div>

				{/* Card Body with a fixed, calculated height that is now structurally accurate. */}
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
					<div className="p-3 h-full w-full overflow-y-auto">{children}</div>
				</div>
			</div>
		</Tooltip.Provider>
	);
}

// --- Sub-components for cleaner rendering (unchanged) ---
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
