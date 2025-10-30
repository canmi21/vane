/* src/components/domain/info-tooltip.tsx */

import React from "react";
import * as Tooltip from "@radix-ui/react-tooltip";
import { motion } from "framer-motion";
import { Info } from "lucide-react";
import { type Plugin } from "~/hooks/use-plugin-data";

interface InfoTooltipProps {
	plugin: Plugin;
}

/**
 * A reusable component that displays a detailed tooltip for a given plugin.
 */
export function InfoTooltip({ plugin }: InfoTooltipProps) {
	const {
		name,
		version,
		author,
		url,
		description,
		input_params,
		output_results,
	} = plugin;

	return (
		<Tooltip.Root>
			<Tooltip.Trigger asChild>
				<button
					className="flex-shrink-0 text-[var(--color-subtext)] transition-colors hover:text-[var(--color-text)] focus:outline-none"
					onMouseDown={(e) => e.stopPropagation()}
					onClick={(e) => e.stopPropagation()}
				>
					<Info size={14} />
				</button>
			</Tooltip.Trigger>
			<Tooltip.Portal>
				<Tooltip.Content
					side="top"
					align="end"
					sideOffset={8}
					// --- FINAL, FINAL FIX 1: Prevent interaction with the tooltip from closing the parent dropdown ---
					onPointerDownOutside={(e) => e.preventDefault()}
					asChild // Use asChild to pass props to the motion.div
				>
					<motion.div
						initial={{ opacity: 0, y: 5 }}
						animate={{ opacity: 1, y: 0 }}
						// --- FINAL, FINAL FIX 2: Apply a high z-index here to ensure it's above the dropdown ---
						className="z-[60] max-w-xs rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg)] p-3 text-xs font-medium text-[var(--color-text)] shadow-lg"
					>
						{/* Header */}
						<div className="flex items-baseline justify-between gap-2 pb-2">
							<h3 className="font-bold capitalize text-[var(--color-text)]">
								{name.replace(/-/g, " ")}
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
									{Object.entries(input_params).map(([key, val]) => (
										<li key={key}>
											{key}: <code {...codeProps}>{val.type}</code>
										</li>
									))}
								</ul>
							</div>
							{/* Outputs Column */}
							<div>
								<h4 {...headingProps}>Output branch:</h4>
								<ul {...listProps}>
									{output_results.tree.map((handle) => (
										<li key={handle}>
											<code {...codeProps}>{handle}</code>
										</li>
									))}
								</ul>

								<h4 {...headingProps} className="mt-2">
									Output Variables
								</h4>
								<ul {...listProps}>
									{Object.keys(output_results.variables).length > 0 ? (
										Object.entries(output_results.variables).map(
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
	);
}

// Shared props for styling
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
