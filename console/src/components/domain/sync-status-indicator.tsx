/* src/components/domain/sync-status-indicator.tsx */

import { Loader2, CheckCircle, AlertCircle, CloudOff } from "lucide-react";
import { type SyncStatus } from "~/hooks/use-canvas-layout";
import { AnimatePresence, motion } from "framer-motion";

interface SyncStatusIndicatorProps {
	status: SyncStatus;
}

const statusConfig = {
	saved: { Icon: CheckCircle, text: "Saved", color: "text-green-500" },
	saving: { Icon: Loader2, text: "Saving...", color: "text-yellow-500" },
	unsaved: {
		Icon: AlertCircle,
		text: "Unsaved changes",
		color: "text-yellow-500",
	},
	error: { Icon: CloudOff, text: "Save failed", color: "text-red-500" },
	loading: { Icon: Loader2, text: "Loading...", color: "text-gray-400" },
	unloaded: { Icon: CloudOff, text: "No domain", color: "text-gray-400" },
};

/**
 * A small indicator to show the current backend synchronization status.
 */
export function SyncStatusIndicator({ status }: SyncStatusIndicatorProps) {
	const { Icon, text, color } = statusConfig[status];
	const isSpinning = status === "saving" || status === "loading";

	return (
		<div className="fixed top-4 right-4 z-20">
			<AnimatePresence mode="wait">
				<motion.div
					key={status}
					initial={{ opacity: 0, y: -10 }}
					animate={{ opacity: 1, y: 0 }}
					exit={{ opacity: 0, y: 10 }}
					transition={{ duration: 0.2 }}
					className={`flex items-center gap-2 rounded-md bg-[var(--color-bg)] px-3 py-1.5 text-xs font-medium shadow-md border border-[var(--color-bg-alt)] ${color}`}
				>
					<Icon size={14} className={isSpinning ? "animate-spin" : ""} />
					<span>{text}</span>
				</motion.div>
			</AnimatePresence>
		</div>
	);
}
