/* src/components/domain/canvas-toolbar.tsx */

import {
	Fullscreen,
	Pencil,
	MapPin,
	Layers2,
	CopyPlus,
	Plus,
} from "lucide-react";
import * as Tooltip from "@radix-ui/react-tooltip";
import { motion } from "framer-motion";

/**
 * A floating toolbar for canvas interactions.
 */
export function CanvasToolbar({ onResetView }: { onResetView: () => void }) {
	// Define the tools for the toolbar
	const tools = [
		{ Icon: Fullscreen, tooltip: "Reset View", action: onResetView },
		{ Icon: Pencil, tooltip: "Edit", action: () => {} },
		{ Icon: MapPin, tooltip: "Pin", action: () => {} },
		{ Icon: Layers2, tooltip: "Layers", action: () => {} },
		{ Icon: CopyPlus, tooltip: "Duplicate", action: () => {} },
		{ Icon: Plus, tooltip: "Add", action: () => {} },
	];

	return (
		// The Radix Tooltip Provider is necessary for the tooltips to function.
		<Tooltip.Provider delayDuration={150}>
			<div className="fixed top-4 left-[calc(50vw+8rem)] -translate-x-1/2 z-10">
				{/* --- MODIFIED: Reduced padding and gap for a more compact look --- */}
				<div className="flex items-center gap-0.5 p-0.5 rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-md">
					{tools.map(({ Icon, tooltip, action }, index) => (
						// --- MODIFIED: Replaced custom tooltip with Radix UI Tooltip ---
						<Tooltip.Root key={index}>
							<Tooltip.Trigger asChild>
								<button
									onClick={action}
									// --- MODIFIED: Reduced button size for compactness ---
									className="flex h-8 w-8 items-center justify-center rounded-md text-[var(--color-subtext)] transition-colors hover:bg-[var(--color-bg-alt)] hover:text-[var(--color-text)] focus:outline-none"
									aria-label={tooltip}
								>
									<Icon size={16} />
								</button>
							</Tooltip.Trigger>
							<Tooltip.Portal>
								<Tooltip.Content sideOffset={8} asChild>
									{/* Use framer-motion for a smooth entrance/exit animation */}
									<motion.div
										initial={{ opacity: 0, y: 5 }}
										animate={{ opacity: 1, y: 0 }}
										exit={{ opacity: 0, y: 5 }}
										transition={{ duration: 0.15 }}
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
