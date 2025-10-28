/* src/components/domain/canvas-node-card.tsx */

import React from "react";
import * as Tooltip from "@radix-ui/react-tooltip";
import { motion } from "framer-motion";

// --- Type Definitions ---
interface OutputHandle {
	label: string;
}

interface CanvasNodeCardProps {
	icon: React.ElementType;
	title: string;
	parameters?: string[];
	outputs: OutputHandle[];
}

/**
 * Generic node card with a body height proportional to the number of outputs.
 */
export function CanvasNodeCard({
	icon: Icon,
	title,
	parameters = [],
	outputs,
}: CanvasNodeCardProps) {
	const headerRef = React.useRef<HTMLDivElement>(null);
	const [headerHeight, setHeaderHeight] = React.useState(41); // Start with a close estimate

	React.useLayoutEffect(() => {
		if (headerRef.current) {
			setHeaderHeight(headerRef.current.offsetHeight);
		}
	}, []);

	const bodyHeight = headerHeight * (outputs.length > 0 ? outputs.length : 1);

	// --- MODIFIED: Calculate the input handle's vertical position ---
	// It should be centered in the body, just like the outputs.
	// Position = top of body (headerHeight) + half of body's height.
	const inputHandleTop = headerHeight + bodyHeight / 2;

	return (
		<Tooltip.Provider delayDuration={150}>
			<div className="relative w-64 rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-md">
				{/* --- MODIFIED: Input Handle now positioned in the center of the body --- */}
				<Tooltip.Root>
					<Tooltip.Trigger asChild>
						<div
							className="absolute left-0 h-3 w-3 rounded-full bg-[var(--color-bg)] border-2 border-[var(--color-theme-border)] cursor-pointer z-10"
							style={{
								top: `${inputHandleTop}px`, // Use the new calculated position
								transform: "translate(-50%, -50%)",
							}}
						/>
					</Tooltip.Trigger>
					<Tooltip.Portal>
						<Tooltip.Content side="left" sideOffset={8} asChild>
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
					{parameters.length > 0 && (
						<Tooltip.Root>
							<Tooltip.Trigger asChild>
								<div className="text-xs font-medium text-[var(--color-subtext)] bg-[var(--color-bg-alt)] px-2 py-0.5 rounded-full cursor-help">
									{`${parameters.length} param input(s)`}
								</div>
							</Tooltip.Trigger>
							<Tooltip.Portal>
								<Tooltip.Content side="top" sideOffset={8} asChild>
									<motion.div
										initial={{ opacity: 0, y: 5 }}
										animate={{ opacity: 1, y: 0 }}
										className="z-50 rounded-md bg-[var(--color-bg-alt)] p-2 text-xs font-medium text-[var(--color-text)] shadow-md"
									>
										<p className="mb-1 font-semibold">Required Parameters:</p>
										<ul className="space-y-1">
											{parameters.map((param) => (
												<li
													key={param}
													className="font-mono text-[var(--color-subtext)]"
												>
													{param}
												</li>
											))}
										</ul>
									</motion.div>
								</Tooltip.Content>
							</Tooltip.Portal>
						</Tooltip.Root>
					)}
				</div>

				{/* Card Body */}
				<div className="relative" style={{ height: `${bodyHeight}px` }}>
					{/* Output handles evenly distributed in the body section */}
					{outputs.map((output, index) => {
						const totalOutputs = outputs.length;
						const positionPercent =
							totalOutputs <= 1 ? 50 : (100 / (totalOutputs + 1)) * (index + 1);

						return (
							<Tooltip.Root key={output.label}>
								<Tooltip.Trigger asChild>
									<div
										className="absolute right-0 h-3 w-3 rounded-full bg-[var(--color-bg)] border-2 border-[var(--color-theme-border)] transition-colors hover:bg-[var(--color-theme-bg)] cursor-pointer"
										style={{
											top: `${positionPercent}%`,
											transform: "translate(50%, -50%)",
										}}
									/>
								</Tooltip.Trigger>
								<Tooltip.Portal>
									<Tooltip.Content side="right" sideOffset={8} asChild>
										<motion.div
											initial={{ opacity: 0, x: 5 }}
											animate={{ opacity: 1, x: 0 }}
											className="z-50 rounded-md bg-[var(--color-bg-alt)] px-2.5 py-1.5 text-xs font-medium text-[var(--color-text)] shadow-md"
										>
											{output.label}
										</motion.div>
									</Tooltip.Content>
								</Tooltip.Portal>
							</Tooltip.Root>
						);
					})}
				</div>
			</div>
		</Tooltip.Provider>
	);
}
