/* src/components/rate-limit/rate-limit-list-card.tsx */

import { ChevronRight, Gauge, HelpCircle, AppWindow } from "lucide-react";
import * as Tooltip from "@radix-ui/react-tooltip";
import { useMemo } from "react";

// --- List Item Component ---
function RateLimitItem({
	domain,
	isSelected,
	onSelect,
}: {
	domain: string;
	isSelected: boolean;
	onSelect: () => void;
}) {
	const isFallback = domain === "fallback";

	const content = (
		<div
			onClick={onSelect}
			className={`flex cursor-pointer items-center justify-between p-4 transition-all hover:bg-[var(--color-theme-bg)] ${isSelected ? "bg-[var(--color-theme-bg)]" : ""}`}
		>
			<div className="flex min-w-0 items-center gap-4">
				{/* --- MODIFIED: Changed icon to AppWindow --- */}
				<AppWindow
					size={20}
					className={
						isSelected
							? "stroke-[var(--color-theme-border)]"
							: "stroke-[var(--color-subtext)]"
					}
				/>
				<p className="truncate font-mono text-sm font-medium text-[var(--color-text)]">
					{domain}
				</p>
				{isFallback && (
					<HelpCircle
						size={14}
						className="flex-shrink-0 stroke-[var(--color-subtext)]"
					/>
				)}
			</div>
			<ChevronRight
				size={18}
				className={`flex-shrink-0 transition-transform ${isSelected ? "translate-x-1" : ""}`}
			/>
		</div>
	);

	if (isFallback) {
		return (
			<Tooltip.Root>
				<Tooltip.Trigger asChild>{content}</Tooltip.Trigger>
				<Tooltip.Portal>
					<Tooltip.Content
						className="z-10 max-w-xs rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg)] p-2 text-xs text-[var(--color-text)] shadow-lg"
						side="top"
						align="start"
						sideOffset={4}
					>
						The fallback policy applies to any request that does not match a
						configured domain.
						<Tooltip.Arrow className="fill-[var(--color-bg-alt)]" />
					</Tooltip.Content>
				</Tooltip.Portal>
			</Tooltip.Root>
		);
	}
	return content;
}

// --- Main List Card Component ---
export function RateLimitListCard({
	domains,
	selectedDomain,
	onSelectDomain,
}: {
	domains: string[];
	selectedDomain: string | null;
	onSelectDomain: (domain: string | null) => void;
}) {
	const sortedDomains = useMemo(() => {
		return [...domains].sort((a, b) => {
			if (a === "fallback") return 1;
			if (b === "fallback") return -1;
			return a.localeCompare(b);
		});
	}, [domains]);

	return (
		<div className="w-full rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-sm">
			<div className="border-b border-[var(--color-bg-alt)] p-6">
				<div className="flex items-center gap-3">
					{/* --- UNCHANGED: Kept Gauge icon for the main card title --- */}
					<Gauge size={20} className="stroke-[var(--color-theme-border)]" />
					<h3 className="text-lg font-semibold text-[var(--color-text)]">
						Rate Limiting Policies
					</h3>
					<span className="rounded-md bg-[var(--color-bg-alt)] px-2 py-0.5 text-xs font-medium text-[var(--color-subtext)]">
						{domains.length}
					</span>
				</div>
			</div>
			<div className="divide-y divide-[var(--color-bg-alt)]">
				{sortedDomains.length > 0 ? (
					sortedDomains.map((domain) => (
						<RateLimitItem
							key={domain}
							domain={domain}
							isSelected={selectedDomain === domain}
							onSelect={() => onSelectDomain(domain)}
						/>
					))
				) : (
					<div className="p-12 text-center text-[var(--color-subtext)]">
						<p className="font-medium">No domains found.</p>
						<p className="text-sm">Configure domains to set rate limits.</p>
					</div>
				)}
			</div>
		</div>
	);
}
