/* src/components/domain/domain-entry-point-card.tsx */

import { Globe } from "lucide-react";

/**
 * A visual representation of the traffic entry point for a specific domain.
 * This is a "start" node with only an output handle.
 */
export function DomainEntryPointCard({ domainName }: { domainName: string }) {
	return (
		// Main card container
		<div className="relative w-64 rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-md">
			{/* Card Header */}
			<div className="flex items-center gap-2 border-b border-[var(--color-bg-alt)] p-3">
				<Globe size={16} className="text-[var(--color-subtext)]" />
				<p className="text-xs font-semibold uppercase tracking-wider text-[var(--color-subtext)]">
					Entry Point
				</p>
			</div>

			{/* Card Body */}
			<div className="p-3">
				<p className="truncate font-medium text-[var(--color-text)]">
					{domainName}
				</p>
			</div>

			{/* Output Handle */}
			{/* This is the connection point for outgoing traffic paths. */}
			<div
				className="absolute right-[-6px] top-1/2 -translate-y-1/2 h-3 w-3 rounded-full bg-[var(--color-bg)] border-2 border-[var(--color-theme-border)] transition-colors hover:bg-[var(--color-theme-bg)] cursor-pointer"
				title="Drag to connect"
			/>
		</div>
	);
}
