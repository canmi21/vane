/* src/components/domain/canvas-toolbar.tsx */

import {
	Fullscreen,
	Spline,
	MapPin,
	Layers2,
	CopyPlus,
	Plus,
	X,
} from "lucide-react";
import * as Tooltip from "@radix-ui/react-tooltip";
import { motion } from "framer-motion";
import React from "react";

interface CanvasToolbarProps {
	onResetView: () => void;
	onFitView: () => void;
	onToggleConnectorMode: () => void;
	isConnectorModeActive: boolean;
	onAddNode: (type: "rate-limit") => void;
	selectedConnectionId: string | null;
	onDeleteSelectedConnection: () => void;
}

export function CanvasToolbar({
	onResetView,
	onFitView,
	onToggleConnectorMode,
	isConnectorModeActive,
	onAddNode,
	selectedConnectionId,
	onDeleteSelectedConnection,
}: CanvasToolbarProps) {
	const connectOrDeleteTool = selectedConnectionId
		? {
				Icon: X,
				tooltip: "Delete Connection",
				action: onDeleteSelectedConnection,
				active: true,
				onMouseDown: (e: React.MouseEvent) => e.stopPropagation(),
			}
		: {
				Icon: Spline,
				tooltip: "Connect Nodes",
				action: onToggleConnectorMode,
				active: isConnectorModeActive,
				onMouseDown: undefined,
			};

	const tools = [
		{
			Icon: Fullscreen,
			tooltip: "Fit to View",
			action: onFitView,
			active: false,
			onMouseDown: undefined,
		},
		connectOrDeleteTool,
		{
			Icon: MapPin,
			tooltip: "Reset View",
			action: onResetView,
			active: false,
			onMouseDown: undefined,
		},
		{
			Icon: Layers2,
			tooltip: "Layers",
			action: () => {},
			active: false,
			onMouseDown: undefined,
		},
		{
			Icon: CopyPlus,
			tooltip: "Duplicate",
			action: () => {},
			active: false,
			onMouseDown: undefined,
		},
		{
			Icon: Plus,
			tooltip: "Add Rate Limit Node",
			action: () => onAddNode("rate-limit"),
			active: false,
			onMouseDown: undefined,
		},
	];

	return (
		<Tooltip.Provider delayDuration={150}>
			<div className="fixed top-4 left-[calc(50vw+8rem)] -translate-x-1/2 z-10">
				<div className="flex items-center gap-0.5 p-0.5 rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-md">
					{tools.map(
						({ Icon, tooltip, action, active, onMouseDown }, index) => {
							const buttonClass = `flex h-8 w-8 items-center justify-center rounded-md text-[var(--color-subtext)] transition-colors focus:outline-none ${
								active
									? "bg-[var(--color-theme-bg)] text-[var(--color-text)]"
									: "hover:bg-[var(--color-bg-alt)] hover:text-[var(--color-text)]"
							}`;

							return (
								<Tooltip.Root key={index}>
									<Tooltip.Trigger asChild>
										<button
											onClick={action}
											onMouseDown={onMouseDown}
											className={buttonClass}
											aria-label={tooltip}
										>
											<Icon size={16} />
										</button>
									</Tooltip.Trigger>
									<Tooltip.Portal>
										<Tooltip.Content sideOffset={8} asChild>
											<motion.div
												initial={{ opacity: 0, y: 5 }}
												animate={{ opacity: 1, y: 0 }}
												className="z-50 rounded-md bg-[var(--color-bg-alt)] px-2.5 py-1.5 text-xs font-medium text-[var(--color-text)] shadow-md"
											>
												{tooltip}
											</motion.div>
										</Tooltip.Content>
									</Tooltip.Portal>
								</Tooltip.Root>
							);
						}
					)}
				</div>
			</div>
		</Tooltip.Provider>
	);
}
