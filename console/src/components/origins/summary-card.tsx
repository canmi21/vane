/* src/components/origins/summary-card.tsx */

import { Target, Server, Lock, Unlock, ShieldAlert } from "lucide-react";
import React from "react";

function SummaryStatCard({
	icon: Icon,
	label,
	value,
}: {
	icon: React.ElementType;
	label: string;
	value: number;
}) {
	return (
		<div className="flex items-center gap-4 rounded-lg bg-[var(--color-bg-alt)] p-4">
			<Icon size={40} className="stroke-[var(--color-subtext)]" />
			<div>
				<div className="text-xs text-[var(--color-subtext)]">{label}</div>
				<div className="text-xl font-bold text-[var(--color-text)]">
					{value}
				</div>
			</div>
		</div>
	);
}

export function SummaryCard({
	stats,
}: {
	stats: { total: number; http: number; https: number; sslSkipped: number };
}) {
	return (
		<div className="rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] p-6 shadow-sm">
			<div className="mb-4 flex items-center gap-3">
				<Target size={20} className="stroke-[var(--color-theme-border)]" />
				<h3 className="text-lg font-semibold text-[var(--color-text)]">
					Origins Overview
				</h3>
			</div>
			<div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
				<SummaryStatCard
					icon={Server}
					label="Total Origins"
					value={stats.total}
				/>
				<SummaryStatCard icon={Lock} label="HTTPS" value={stats.https} />
				<SummaryStatCard icon={Unlock} label="HTTP" value={stats.http} />
				<SummaryStatCard
					icon={ShieldAlert}
					label="SSL Skipped"
					value={stats.sslSkipped}
				/>
			</div>
		</div>
	);
}
