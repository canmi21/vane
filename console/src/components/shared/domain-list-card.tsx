/* src/components/shared/domain-list-card.tsx */

import { AppWindow, ChevronRight, HelpCircle } from "lucide-react";
import * as Tooltip from "@radix-ui/react-tooltip";
import React from "react";

// --- Data structure for items passed to the list card ---
export interface DomainListItem {
	domain: string;
	badge?: {
		icon?: React.ElementType;
		text: string;
	};
}

// --- Reusable List Item Component ---
function DomainItem({
	item,
	isSelected,
	onSelect,
}: {
	item: DomainListItem;
	isSelected: boolean;
	onSelect: () => void;
}) {
	const isFallback = item.domain === "fallback";

	const content = (
		<div
			onClick={onSelect}
			className={`flex cursor-pointer items-center justify-between p-4 transition-all hover:bg-[var(--color-theme-bg)] ${
				isSelected ? "bg-[var(--color-theme-bg)]" : ""
			}`}
		>
			<div className="flex min-w-0 items-center gap-4">
				<AppWindow
					size={20}
					className={
						isSelected
							? "stroke-[var(--color-theme-border)]"
							: "stroke-[var(--color-subtext)]"
					}
				/>
				<p className="truncate font-mono text-sm font-medium text-[var(--color-text)]">
					{item.domain}
				</p>
				{isFallback && (
					<HelpCircle
						size={14}
						className="flex-shrink-0 stroke-[var(--color-subtext)]"
					/>
				)}
			</div>
			<div className="flex flex-shrink-0 items-center gap-3">
				{item.badge && (
					<div className="flex items-center gap-2 rounded-md bg-[var(--color-bg-alt)] px-2.5 py-1 text-xs font-medium text-[var(--color-subtext)]">
						{item.badge.icon && <item.badge.icon size={14} />}
						<span>{item.badge.text}</span>
					</div>
				)}
				<ChevronRight
					size={18}
					className={`transition-transform ${
						isSelected ? "translate-x-1" : ""
					}`}
				/>
			</div>
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

// --- Main Generic Domain List Card Component ---
export function DomainListCard({
	title,
	icon: Icon,
	items,
	selectedDomain,
	onSelectDomain,
}: {
	title: string;
	icon: React.ElementType;
	items: DomainListItem[];
	selectedDomain: string | null;
	onSelectDomain: (domain: string | null) => void;
}) {
	// --- FIX: Removed the internal sorting logic. ---
	// The component now trusts the order of the `items` prop provided by the parent.

	return (
		<div className="w-full rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-sm">
			<div className="border-b border-[var(--color-bg-alt)] p-6">
				<div className="flex items-center gap-3">
					<Icon size={20} className="stroke-[var(--color-theme-border)]" />
					<h3 className="text-lg font-semibold text-[var(--color-text)]">
						{title}
					</h3>
					<span className="rounded-md bg-[var(--color-bg-alt)] px-2 py-0.5 text-xs font-medium text-[var(--color-subtext)]">
						{items.length}
					</span>
				</div>
			</div>
			<div className="overflow-hidden rounded-b-xl divide-y divide-[var(--color-bg-alt)]">
				{items.length > 0 ? (
					// Directly map over the 'items' prop.
					items.map((item) => (
						<DomainItem
							key={item.domain}
							item={item}
							isSelected={selectedDomain === item.domain}
							onSelect={() => onSelectDomain(item.domain)}
						/>
					))
				) : (
					<div className="p-12 text-center text-[var(--color-subtext)]">
						<p className="font-medium">No domains found.</p>
						<p className="text-sm">Configure domains to set policies.</p>
					</div>
				)}
			</div>
		</div>
	);
}
