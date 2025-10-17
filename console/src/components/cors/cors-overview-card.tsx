/* src/components/cors/cors-overview-card.tsx */

import {
	AppWindow,
	ArrowRightLeft,
	Route,
	RouteOff,
	AlertTriangle,
} from "lucide-react";
import React from "react";

// --- Internal Stat Card Component ---
function OverviewStatCard({
	icon: Icon,
	label,
	value,
}: {
	icon: React.ElementType;
	label: string;
	value: string | number;
}) {
	return (
		<div className="flex items-center gap-4 rounded-lg bg-[var(--color-bg-alt)] p-4">
			<Icon size={32} className="stroke-[var(--color-subtext)]" />
			<div>
				<div className="text-xs text-[var(--color-subtext)]">{label}</div>
				<div className="text-xl font-bold text-[var(--color-text)]">
					{value}
				</div>
			</div>
		</div>
	);
}

// --- Main Overview Card Component ---
export type CorsOverviewStats = {
	total: number;
	proxyHandled: number;
	originHandled: number;
	wildcardOrigins: number;
};

export function CorsOverviewCard({ stats }: { stats: CorsOverviewStats }) {
	return (
		<div className="rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] p-6 shadow-sm">
			<div className="mb-4 flex items-center gap-3">
				<ArrowRightLeft
					size={20}
					className="stroke-[var(--color-theme-border)]"
				/>
				<h3 className="text-lg font-semibold text-[var(--color-text)]">
					CORS Overview
				</h3>
			</div>
			<div className="grid grid-cols-2 gap-4 md:grid-cols-4">
				<OverviewStatCard
					icon={AppWindow}
					label="Total Policies"
					value={stats.total}
				/>
				<OverviewStatCard
					icon={Route}
					label="Handled by Vane"
					value={stats.proxyHandled}
				/>
				<OverviewStatCard
					icon={RouteOff}
					label="Passed to Origin"
					value={stats.originHandled}
				/>
				<OverviewStatCard
					icon={AlertTriangle}
					label="Wildcard Origins"
					value={stats.wildcardOrigins}
				/>
			</div>
		</div>
	);
}
