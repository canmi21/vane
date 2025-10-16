/* src/components/certs/cert-overview-card.tsx */

import {
	ShieldCheck,
	Shield,
	ShieldX,
	FileBadge,
	CalendarClock,
	Users,
	Fingerprint,
	UserCheck,
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
export type CertOverviewStats = {
	total: number;
	valid: number;
	expired: number;
	soonestExpiryDays: number | null;
	uniqueFormats: number;
	uniqueIssuers: number;
	selfSigned: number;
	uniqueAlgorithms: number;
};

export function CertOverviewCard({ stats }: { stats: CertOverviewStats }) {
	return (
		<div className="rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] p-6 shadow-sm">
			<div className="mb-4 flex items-center gap-3">
				<Shield size={20} className="stroke-[var(--color-theme-border)]" />
				<h3 className="text-lg font-semibold text-[var(--color-text)]">
					Certificates Overview
				</h3>
			</div>
			<div className="grid grid-cols-2 gap-4 md:grid-cols-4">
				<OverviewStatCard
					icon={Shield}
					label="Total Certificates"
					value={stats.total}
				/>
				<OverviewStatCard
					icon={ShieldCheck}
					label="Valid Certificates"
					value={stats.valid}
				/>
				<OverviewStatCard
					icon={ShieldX}
					label="Expired Certificates"
					value={stats.expired}
				/>
				<OverviewStatCard
					icon={CalendarClock}
					label="Soonest Expiry"
					value={
						stats.soonestExpiryDays !== null
							? `${stats.soonestExpiryDays} days`
							: "N/A"
					}
				/>
				<OverviewStatCard
					icon={Users}
					label="Unique Issuers (CAs)"
					value={stats.uniqueIssuers}
				/>
				<OverviewStatCard
					icon={UserCheck}
					label="Self-Signed Certs"
					value={stats.selfSigned}
				/>
				<OverviewStatCard
					icon={Fingerprint}
					label="Unique Algorithms"
					value={stats.uniqueAlgorithms}
				/>
				<OverviewStatCard
					icon={FileBadge}
					label="Unique Formats"
					value={stats.uniqueFormats}
				/>
			</div>
		</div>
	);
}
