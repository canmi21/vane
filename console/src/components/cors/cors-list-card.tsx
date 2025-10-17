/* src/components/cors/cors-list-card.tsx */

import { Shield, ChevronRight, Server, ShieldCheck } from "lucide-react";
import { type CorsStatus } from "~/routes/$instance/cors-management/";

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

	return (
		<div
			onClick={onSelect}
			className={`flex cursor-pointer items-center justify-between p-4 transition-all hover:bg-[var(--color-theme-bg)] ${
				isSelected ? "bg-[var(--color-theme-bg)]" : ""
			}`}
		>
			<div className="flex min-w-0 items-center gap-4">
				<Shield
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
			</div>
			<div className="flex flex-shrink-0 items-center gap-3">
				<div className="flex items-center gap-2 rounded-md bg-[var(--color-bg-alt)] px-2.5 py-1 text-xs font-medium text-[var(--color-subtext)]">
					{isProxyHandled ? <ShieldCheck size={14} /> : <Server size={14} />}
					<span>{isProxyHandled ? "Vane Proxy" : "Origin Server"}</span>
				</div>
				<ChevronRight
					size={18}
					className={`transition-transform ${isSelected ? "translate-x-1" : ""}`}
				/>
			</div>
		</div>
	);
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
	return (
		<div className="w-full rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-sm">
			<div className="border-b border-[var(--color-bg-alt)] p-6">
				<div className="flex items-center gap-3">
					<Shield size={20} className="stroke-[var(--color-theme-border)]" />
					<h3 className="text-lg font-semibold text-[var(--color-text)]">
						Domain CORS Policies
					</h3>
					<span className="rounded-md bg-[var(--color-bg-alt)] px-2 py-0.5 text-xs font-medium text-[var(--color-subtext)]">
						{statuses.length}
					</span>
				</div>
			</div>
			<div className="divide-y divide-[var(--color-bg-alt)]">
				{statuses.length > 0 ? (
					statuses.map((status) => (
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
