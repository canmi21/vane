/* src/components/domain/canvas-toolbar.tsx */

import {
	Fullscreen,
	Spline,
	MapPin,
	Layers2,
	CopyPlus,
	Plus,
} from "lucide-react";
import * as Tooltip from "@radix-ui/react-tooltip";
import { motion } from "framer-motion";

interface CanvasToolbarProps {
	onResetView: () => void;
	onFitView: () => void;
	onToggleConnectorMode: () => void;
	isConnectorModeActive: boolean;
	onAddNode: (type: "rate-limit") => void;
}

export function CanvasToolbar({
	onResetView,
	onFitView,
	onToggleConnectorMode,
	isConnectorModeActive,
	onAddNode,
}: CanvasToolbarProps) {
	const tools = [
		{ Icon: Fullscreen, tooltip: "Fit to View", action: onFitView },
		{
			Icon: Spline,
			tooltip: "Connect Nodes",
			action: onToggleConnectorMode,
			active: isConnectorModeActive,
		},
		{ Icon: MapPin, tooltip: "Reset View", action: onResetView },
		{ Icon: Layers2, tooltip: "Layers", action: () => {} },
		{ Icon: CopyPlus, tooltip: "Duplicate", action: () => {} },
		{
			Icon: Plus,
			tooltip: "Add Rate Limit Node",
			action: () => onAddNode("rate-limit"),
		},
	];
	return (
		<Tooltip.Provider delayDuration={150}>
			<div className="fixed top-4 left-[calc(50vw+8rem)] -translate-x-1/2 z-10">
				<div className="flex items-center gap-0.5 p-0.5 rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-md">
					{tools.map(({ Icon, tooltip, action, active }, index) => (
						<Tooltip.Root key={index}>
							<Tooltip.Trigger asChild>
								<button
									onClick={action}
									className={`flex h-8 w-8 items-center justify-center rounded-md text-[var(--color-subtext)] transition-colors hover:bg-[var(--color-bg-alt)] hover:text-[var(--color-text)] focus:outline-none ${active ? "bg-[var(--color-theme-bg)] text-[var(--color-text)]" : ""}`}
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
					))}
				</div>
			</div>
		</Tooltip.Provider>
	);
}
