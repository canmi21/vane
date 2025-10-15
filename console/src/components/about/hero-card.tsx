/* src/components/about/hero-card.tsx */

import React from "react";
import {
	Activity,
	Calendar,
	GitCommitHorizontal,
	Info,
	Package,
} from "lucide-react";
import VaneLogo from "~/assets/about.svg";
import {
	type RootInfo,
	type InstanceInfo,
} from "~/routes/$instance/about/index";

// --- Helper Functions ---
const parseVersion = (vaneBuild: string = "") =>
	vaneBuild.split(" ")[1] ?? "N/A";
const parseBuildHash = (vaneBuild: string = "") =>
	vaneBuild.split(" ")[2]?.slice(1, -1) ?? "N/A";
const formatDate = (dateString: string) => {
	try {
		return new Date(dateString).toLocaleDateString("en-US", {
			year: "numeric",
			month: "short",
			day: "numeric",
		});
	} catch {
		return "N/A";
	}
};

// --- Main Hero Card Component ---
export function HeroCard({
	rootData,
	instanceData,
	isActive,
}: {
	rootData?: RootInfo | null;
	instanceData?: InstanceInfo | null;
	isActive: boolean;
}) {
	return (
		<div className="relative overflow-hidden rounded-2xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] p-8 shadow-lg">
			<div className="absolute right-0 top-0 h-48 w-48 translate-x-16 -translate-y-16 rounded-full bg-[var(--color-theme-bg)] blur-3xl" />
			<div className="relative">
				<div className="mb-6 flex flex-wrap items-start justify-between gap-4">
					<div className="flex items-center gap-5">
						<div className="flex h-20 w-20 items-center justify-center p-2">
							<img
								src={VaneLogo}
								alt="Vane Logo"
								className="h-full w-full"
								style={{ animation: "spin 15s linear infinite" }}
							/>
						</div>
						<div>
							<h1 className="text-3xl font-bold text-[var(--color-text)]">
								Vane Engine
							</h1>
							<div className="mt-1 max-w-lg text-sm text-[var(--color-subtext)]">
								<p>Flow-based. Event-driven. Rust-native.</p>
								<p className="italic">
									Like a dandelion carried by the wind, it follows direction yet
									defines its own.
								</p>
							</div>
						</div>
					</div>
					{isActive && (
						<div className="flex flex-shrink-0 items-center gap-2 rounded-full border-2 border-[var(--color-theme-border)] bg-[var(--color-theme-bg)] px-4 py-2 text-sm font-semibold text-[var(--color-text)]">
							<Activity
								size={16}
								className="stroke-[var(--color-theme-border)]"
							/>
							Running
						</div>
					)}
				</div>
				<div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4">
					<StatCard
						icon={Package}
						label="Version"
						value={parseVersion(rootData?.build.vane)}
						accent
					/>
					<StatCard
						icon={GitCommitHorizontal}
						label="Build Hash"
						value={parseBuildHash(rootData?.build.vane)}
					/>
					<StatCard
						icon={Info}
						label="Instance ID"
						value={instanceData?.instance_id.slice(0, 8).toUpperCase() || "N/A"}
					/>
					<StatCard
						icon={Calendar}
						label="Created"
						value={formatDate(instanceData?.created_at || "")}
					/>
				</div>
			</div>
		</div>
	);
}

// --- Reusable StatCard (co-located) ---
function StatCard({
	icon: Icon,
	label,
	value,
	accent = false,
}: {
	icon: React.ElementType;
	label: string;
	value: string;
	accent?: boolean;
}) {
	return (
		<div
			className={`flex min-h-[72px] items-center rounded-xl border p-4 transition-all hover:shadow-md ${accent ? "border-[var(--color-theme-border)] bg-[var(--color-theme-bg)]" : "border-[var(--color-bg-alt)] bg-[var(--color-bg-alt)]"}`}
		>
			<div className="flex items-center gap-4">
				<div className="flex h-full items-center">
					<Icon
						size={28}
						className={
							accent
								? "stroke-[var(--color-theme-border)]"
								: "stroke-[var(--color-subtext)]"
						}
					/>
				</div>
				<div className="flex flex-col">
					<span className="text-xs text-[var(--color-subtext)]">{label}</span>
					<span className="font-mono text-base font-semibold text-[var(--color-text)]">
						{value}
					</span>
				</div>
			</div>
		</div>
	);
}
