/* src/components/domain/canvas-node-card.tsx */

import React from "react";
import * as Tooltip from "@radix-ui/react-tooltip";
import { motion } from "framer-motion";
import { type NodeHandle } from "~/lib/canvas-layout";
import { Info } from "lucide-react";
import {
	type ParamDefinition,
	type VariableDefinition,
} from "~/hooks/use-plugin-data";

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
	version?: string;
	description?: string;
	author?: string;
	url?: string;
	inputParams?: Record<string, ParamDefinition>;
	outputHandles?: string[];
	outputVariables?: Record<string, VariableDefinition>;
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
	version,
	description,
	author,
	url,
	inputParams,
	outputHandles,
	outputVariables,
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

	const hasInfo = !!description;

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

					{hasInfo && (
						<Tooltip.Root>
							<Tooltip.Trigger asChild>
								<button
									className="flex-shrink-0 text-[var(--color-subtext)] transition-colors hover:text-[var(--color-text)] focus:outline-none"
									onMouseDown={(e) => e.stopPropagation()}
								>
									<Info size={14} />
								</button>
							</Tooltip.Trigger>
							<Tooltip.Portal>
								<Tooltip.Content side="top" align="end" sideOffset={8}>
									<motion.div
										initial={{ opacity: 0, y: 5 }}
										animate={{ opacity: 1, y: 0 }}
										className="z-50 max-w-xs rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg)] p-3 text-xs font-medium text-[var(--color-text)] shadow-lg"
									>
										{/* Header */}
										<div className="flex items-baseline justify-between gap-2 pb-2">
											<h3 className="font-bold capitalize text-[var(--color-text)]">
												{title}
												<span className="ml-1.5 text-xs font-normal text-[var(--color-subtext)]">
													{version}
												</span>
											</h3>
											{author && url && (
												<a {...linkProps} href={url}>
													@{author}
												</a>
											)}
										</div>

										{/* Description */}
										<p className="text-[var(--color-subtext)]">{description}</p>

										{/* IO Details */}
										<div className="mt-3 grid grid-cols-2 gap-3 pt-3 border-t border-[var(--color-bg-alt)]">
											{/* Inputs Column */}
											<div>
												<h4 {...headingProps}>Input Params</h4>
												<ul {...listProps}>
													{inputParams &&
														Object.entries(inputParams).map(([key, val]) => (
															<li key={key}>
																{key}: <code {...codeProps}>{val.type}</code>
															</li>
														))}
												</ul>
											</div>
											{/* Outputs Column */}
											<div>
												{/* --- FINAL, FINAL FIX 1: Update heading and rendering logic --- */}
												<h4 {...headingProps}>Output branch:</h4>
												<ul {...listProps}>
													{outputHandles?.map((handle) => (
														<li key={handle}>
															<code {...codeProps}>{handle}</code>
														</li>
													))}
												</ul>

												{/* --- FINAL, FINAL FIX 2: Always show variables, with a fallback --- */}
												<h4 {...headingProps} className="mt-2">
													Output Variables
												</h4>
												<ul {...listProps}>
													{outputVariables &&
													Object.keys(outputVariables).length > 0 ? (
														Object.entries(outputVariables).map(
															([key, val]) => (
																<li key={key}>
																	{key}: <code {...codeProps}>{val.type}</code>
																</li>
															)
														)
													) : (
														<li>None</li>
													)}
												</ul>
											</div>
										</div>
									</motion.div>
								</Tooltip.Content>
							</Tooltip.Portal>
						</Tooltip.Root>
					)}
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
				className={`absolute h-3 w-3 rounded-full bg-[var(--color-bg)] border-2 border-[var(--color-theme-border)] z-10 transition-colors ${isConnecting ? "cursor-crosshair hover:bg-[var(--color-theme-bg)]" : "cursor-pointer"}`}
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

// Shared props for styling the detailed info tooltip
const headingProps = {
	className: "font-semibold text-[var(--color-text)] mb-1",
};
const listProps = { className: "space-y-0.5 text-[var(--color-subtext)]" };
const codeProps = {
	className:
		"px-1 py-0.5 rounded bg-[var(--color-bg-alt)] text-xs inline-block",
};
const linkProps = {
	target: "_blank",
	rel: "noopener noreferrer",
	className: "text-[var(--color-theme-border)] hover:underline",
	onClick: (e: React.MouseEvent) => e.stopPropagation(),
};
