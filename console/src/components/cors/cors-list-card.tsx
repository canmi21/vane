/* src/components/cors/cors-list-card.tsx */

import {
	Globe,
	ChevronRight,
	Route,
	RouteOff,
	HelpCircle,
	AppWindow,
} from "lucide-react";
import * as Tooltip from "@radix-ui/react-tooltip";
import { type CorsStatus } from "~/routes/$instance/cors-management/";
import { useMemo } from "react";

// --- List Item Component ---
function CorsItem({
	status,
	isSelected,
	onSelect,
}: {
	status: CorsStatus;
	isSelected: boolean;
	onSelect: () => void;
}) {
	const isProxyHandled = status.preflight_handling === "proxy_decision";
	const isFallback = status.domain === "fallback";

	const content = (
		<div
			onClick={onSelect}
			className={`flex cursor-pointer items-center justify-between p-4 transition-all hover:bg-[var(--color-theme-bg)] ${isSelected ? "bg-[var(--color-theme-bg)]" : ""}`}
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
					{status.domain}
				</p>
				{isFallback && (
					<HelpCircle
						size={14}
						className="flex-shrink-0 stroke-[var(--color-subtext)]"
					/>
				)}
			</div>
			<div className="flex flex-shrink-0 items-center gap-3">
				<div className="flex items-center gap-2 rounded-md bg-[var(--color-bg-alt)] px-2.5 py-1 text-xs font-medium text-[var(--color-subtext)]">
					{isProxyHandled ? <Route size={14} /> : <RouteOff size={14} />}
					<span>{isProxyHandled ? "Vane Proxy" : "Origin Server"}</span>
				</div>
				<ChevronRight
					size={18}
					className={`transition-transform ${isSelected ? "translate-x-1" : ""}`}
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

// --- Main List Card Component ---
export function CorsListCard({
	statuses,
	selectedDomain,
	onSelectDomain,
}: {
	statuses: CorsStatus[];
	selectedDomain: string | null;
	onSelectDomain: (domain: string | null) => void;
}) {
	// --- Sort statuses to ensure "fallback" is always last ---
	const sortedStatuses = useMemo(() => {
		return [...statuses].sort((a, b) => {
			if (a.domain === "fallback") return 1;
			if (b.domain === "fallback") return -1;
			return a.domain.localeCompare(b.domain);
		});
	}, [statuses]);

	return (
		<div className="w-full rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-sm">
			<div className="border-b border-[var(--color-bg-alt)] p-6">
				<div className="flex items-center gap-3">
					<Globe size={20} className="stroke-[var(--color-theme-border)]" />
					<h3 className="text-lg font-semibold text-[var(--color-text)]">
						Domain CORS Policies
					</h3>
					<span className="rounded-md bg-[var(--color-bg-alt)] px-2 py-0.5 text-xs font-medium text-[var(--color-subtext)]">
						{statuses.length}
					</span>
				</div>
			</div>
			<div className="divide-y divide-[var(--color-bg-alt)]">
				{sortedStatuses.length > 0 ? (
					sortedStatuses.map((status) => (
						<CorsItem
							key={status.domain}
							status={status}
							isSelected={selectedDomain === status.domain}
							onSelect={() => onSelectDomain(status.domain)}
						/>
					))
				) : (
					<div className="p-12 text-center text-[var(--color-subtext)]">
						<p className="font-medium">No domains found.</p>
						<p className="text-sm">
							CORS policies are available for each configured domain.
						</p>
					</div>
				)}
			</div>
		</div>
	);
}
