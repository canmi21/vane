/* src/components/cors/cors-overview-card.tsx */

import { Shield, Globe, ShieldCheck, Server } from "lucide-react";
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
};

export function CorsOverviewCard({ stats }: { stats: CorsOverviewStats }) {
	return (
		<div className="rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] p-6 shadow-sm">
			<div className="mb-4 flex items-center gap-3">
				<Shield size={20} className="stroke-[var(--color-theme-border)]" />
				<h3 className="text-lg font-semibold text-[var(--color-text)]">
					CORS Overview
				</h3>
			</div>
			<div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
				<OverviewStatCard
					icon={Globe}
					label="Total Configured Domains"
					value={stats.total}
				/>
				<OverviewStatCard
					icon={ShieldCheck}
					label="Handled by Vane Proxy"
					value={stats.proxyHandled}
				/>
				<OverviewStatCard
					icon={Server}
					label="Passed to Origin Server"
					value={stats.originHandled}
				/>
			</div>
		</div>
	);
}
