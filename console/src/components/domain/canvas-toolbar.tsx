/* src/components/domain/canvas-toolbar.tsx */

import {
	Fullscreen,
	Spline,
	MapPin,
	Plus,
	X,
	FileWarning,
	Send,
} from "lucide-react";
import * as Tooltip from "@radix-ui/react-tooltip";
import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { motion } from "framer-motion";
import { type Plugin } from "~/hooks/use-plugin-data";
import React from "react";
import { InfoTooltip } from "./info-tooltip";

interface CanvasToolbarProps {
	plugins: Plugin[];
	onResetView: () => void;
	onFitView: () => void;
	onToggleConnectorMode: () => void;
	isConnectorModeActive: boolean;
	onAddNode: (plugin: Plugin) => void;
	onAddErrorPageNode: () => void;
	onAddReturnResponseNode: () => void; // --- FINAL FIX: Add prop for the new action ---
	selectedConnectionId: string | null;
	onDeleteSelectedConnection: () => void;
	selectedNodeId: string | null;
	onDeleteSelectedNode: () => void;
}

export function CanvasToolbar({
	plugins,
	onResetView,
	onFitView,
	onToggleConnectorMode,
	isConnectorModeActive,
	onAddNode,
	onAddErrorPageNode,
	onAddReturnResponseNode, // --- FINAL FIX: Destructure new prop ---
	selectedConnectionId,
	onDeleteSelectedConnection,
	selectedNodeId,
	onDeleteSelectedNode,
}: CanvasToolbarProps) {
	const isDeletableNodeSelected =
		selectedNodeId && selectedNodeId !== "entry-point";
	const isConnectionSelected = !!selectedConnectionId;

	let connectOrDeleteTool;
	if (isDeletableNodeSelected) {
		connectOrDeleteTool = {
			Icon: X,
			tooltip: "Delete Node",
			action: onDeleteSelectedNode,
			active: true,
		};
	} else if (isConnectionSelected) {
		connectOrDeleteTool = {
			Icon: X,
			tooltip: "Delete Connection",
			action: onDeleteSelectedConnection,
			active: true,
		};
	} else {
		connectOrDeleteTool = {
			Icon: Spline,
			tooltip: "Connect Nodes",
			action: onToggleConnectorMode,
			active: isConnectorModeActive,
		};
	}

	const mainTools = [
		{
			Icon: Fullscreen,
			tooltip: "Fit to View",
			action: onFitView,
			active: false,
		},
		connectOrDeleteTool,
		{ Icon: MapPin, tooltip: "Reset View", action: onResetView, active: false },
	];

	return (
		<Tooltip.Provider delayDuration={150}>
			<div className="fixed top-4 left-1/2 -translate-x-1/2 z-10">
				<div className="flex items-center gap-0.5 p-0.5 rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-md">
					{/* Render all main tools */}
					{mainTools.map(({ Icon, tooltip, action, active }, index) => (
						<Tooltip.Root key={index}>
							<Tooltip.Trigger asChild>
								<button
									onMouseDown={(e: React.MouseEvent) => {
										e.stopPropagation();
										action();
									}}
									className={`flex h-8 w-8 items-center justify-center rounded-md text-[var(--color-subtext)] transition-colors focus:outline-none ${
										active
											? "bg-[var(--color-theme-bg)] text-[var(--color-text)]"
											: "hover:bg-[var(--color-bg-alt)] hover:text-[var(--color-text)]"
									}`}
									aria-label={tooltip}
								>
									<Icon size={16} />
								</button>
							</Tooltip.Trigger>
							<Tooltip.Portal>
								<Tooltip.Content sideOffset={8} asChild>
									<motion.div {...TooltipAnimation}>{tooltip}</motion.div>
								</Tooltip.Content>
							</Tooltip.Portal>
						</Tooltip.Root>
					))}

					<DropdownMenu.Root>
						<Tooltip.Root>
							<Tooltip.Trigger asChild>
								<DropdownMenu.Trigger asChild>
									<button
										onMouseDown={(e: React.MouseEvent) => e.stopPropagation()}
										className="flex h-8 w-8 items-center justify-center rounded-md text-[var(--color-subtext)] transition-colors hover:bg-[var(--color-bg-alt)] hover:text-[var(--color-text)] focus:outline-none"
										aria-label="Add Node"
									>
										<Plus size={16} />
									</button>
								</DropdownMenu.Trigger>
							</Tooltip.Trigger>
							<Tooltip.Portal>
								<Tooltip.Content sideOffset={8} asChild>
									<motion.div {...TooltipAnimation}>Add Node</motion.div>
								</Tooltip.Content>
							</Tooltip.Portal>
						</Tooltip.Root>

						<DropdownMenu.Portal>
							<DropdownMenu.Content
								sideOffset={8}
								className="z-50 min-w-[240px] rounded-md border border-[var(--color-bg-alt)] bg-[var(--color-bg)] p-1 shadow-md"
								onCloseAutoFocus={(e) => e.preventDefault()}
							>
								<DropdownMenu.Label className="px-2 py-1.5 text-xs text-[var(--color-subtext)]">
									Built-in Actions
								</DropdownMenu.Label>
								<DropdownMenu.Item
									onSelect={onAddErrorPageNode}
									className="relative flex cursor-pointer select-none items-center gap-2 rounded-sm px-2 py-1.5 text-sm text-[var(--color-text)] outline-none hover:bg-[var(--color-theme-bg)]"
								>
									<FileWarning size={14} />
									Return Error Page
								</DropdownMenu.Item>
								{/* --- FINAL FIX: Add the new menu item --- */}
								<DropdownMenu.Item
									onSelect={onAddReturnResponseNode}
									className="relative flex cursor-pointer select-none items-center gap-2 rounded-sm px-2 py-1.5 text-sm text-[var(--color-text)] outline-none hover:bg-[var(--color-theme-bg)]"
								>
									<Send size={14} />
									Return Response
								</DropdownMenu.Item>
								<DropdownMenu.Separator className="my-1 h-px bg-[var(--color-bg-alt)]" />
								<DropdownMenu.Label className="px-2 py-1.5 text-xs text-[var(--color-subtext)]">
									Available Plugins
								</DropdownMenu.Label>
								<DropdownMenu.Separator className="my-1 h-px bg-[var(--color-bg-alt)]" />
								{plugins.map((plugin) => (
									<DropdownMenu.Item
										key={`${plugin.name}-${plugin.version}`}
										onSelect={() => onAddNode(plugin)}
										className="relative flex cursor-pointer select-none items-center justify-between rounded-sm px-2 py-1.5 text-sm text-[var(--color-text)] outline-none hover:bg-[var(--color-theme-bg)]"
									>
										<div className="flex items-center gap-2">
											<span className="capitalize">
												{plugin.name.replace(/-/g, " ")}
												<span className="ml-1 text-xs text-[var(--color-subtext)]">
													{plugin.version}
												</span>
											</span>
											<span
												className={`rounded px-1.5 py-0.5 text-xs font-semibold ${
													plugin.interface.type === "internal"
														? "bg-blue-500/10 text-blue-400"
														: "bg-purple-500/10 text-purple-400"
												}`}
											>
												{plugin.interface.type}
											</span>
										</div>

										<div className="pr-1">
											<InfoTooltip plugin={plugin} />
										</div>
									</DropdownMenu.Item>
								))}
								{plugins.length === 0 && (
									<div className="px-2 py-1.5 text-center text-xs text-[var(--color-subtext)]">
										No plugins found.
									</div>
								)}
							</DropdownMenu.Content>
						</DropdownMenu.Portal>
					</DropdownMenu.Root>
				</div>
			</div>
		</Tooltip.Provider>
	);
}

const TooltipAnimation = {
	initial: { opacity: 0, y: 5 },
	animate: { opacity: 1, y: 0 },
	className:
		"z-50 rounded-md bg-[var(--color-bg-alt)] px-2.5 py-1.5 text-xs font-medium text-[var(--color-text)] shadow-md",
};
