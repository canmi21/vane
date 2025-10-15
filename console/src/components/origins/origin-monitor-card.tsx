/* src/components/origins/origin-monitor-card.tsx */

import { useState, useMemo } from "react";
import { Activity, RefreshCw, ChevronDown, HeartPulse } from "lucide-react";
import { motion, AnimatePresence } from "framer-motion";
import {
	type UseQueryResult,
	type UseMutationResult,
} from "@tanstack/react-query";
import { type RequestResult } from "~/api/request";
import { type OriginResponse } from "~/routes/$instance/origins/";
import {
	type MonitorReportsStore,
	type TaskStatus,
} from "~/routes/$instance/origins/";
import { MonitorItem, type MonitoredOrigin } from "./monitor-item";

function formatNextCheckTime(
	nextCheck: string | null | undefined,
	taskStatus: TaskStatus | undefined
): string {
	if (taskStatus === "running") return "In progress...";
	if (!nextCheck) return "Scheduled soon...";
	try {
		const nextDate = new Date(nextCheck);
		const now = new Date();
		const diffSeconds = Math.round((nextDate.getTime() - now.getTime()) / 1000);
		if (diffSeconds <= 0) return "Due now...";
		return `In ~${diffSeconds}s`;
	} catch {
		return "Calculating...";
	}
}

export function OriginMonitorCard({
	origins,
	monitorReports,
	taskStatusQuery,
	nextCheckQuery,
	triggerCheckMutation,
}: {
	origins: OriginResponse[];
	monitorReports: MonitorReportsStore;
	taskStatusQuery: UseQueryResult<RequestResult<TaskStatus>>;
	nextCheckQuery: UseQueryResult<RequestResult<string | null>>;
	triggerCheckMutation: UseMutationResult<RequestResult<unknown>, Error, void>;
}) {
	const [isExpanded, setIsExpanded] = useState(false);

	const monitoredOrigins = useMemo<MonitoredOrigin[]>(() => {
		return origins.map((origin) => ({
			...origin,
			report: monitorReports[origin.id],
		}));
	}, [origins, monitorReports]);

	const { healthy, unhealthy, pending } = useMemo(() => {
		const healthy: MonitoredOrigin[] = [];
		const unhealthy: MonitoredOrigin[] = [];
		const pending: MonitoredOrigin[] = [];

		for (const item of monitoredOrigins) {
			const status = item.report?.status ?? "pending";
			if (status === "healthy") {
				healthy.push(item);
			} else if (status === "unhealthy") {
				unhealthy.push(item);
			} else {
				pending.push(item);
			}
		}
		// Sort unhealthy items to the top
		return { healthy, unhealthy, pending };
	}, [monitoredOrigins]);

	const taskStatus = taskStatusQuery.data?.data;
	const isChecking = taskStatus === "running" || triggerCheckMutation.isPending;

	return (
		<div className="rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-sm">
			{/* Header */}
			<div className="flex items-center justify-between p-6">
				<div className="flex items-center gap-3">
					<Activity size={20} className="stroke-[var(--color-theme-border)]" />
					<h3 className="text-lg font-semibold text-[var(--color-text)]">
						Origin Health Monitor
					</h3>
					{unhealthy.length > 0 && (
						<div className="flex items-center gap-1.5 rounded-full bg-[var(--color-bg-alt)] px-2.5 py-1 text-xs font-semibold text-[var(--color-subtext)]">
							<HeartPulse size={14} />
							<span>{unhealthy.length} Unhealthy</span>
						</div>
					)}
				</div>
				<div className="flex items-center gap-3">
					<div className="text-right text-xs">
						<div className="text-[var(--color-subtext)]">Next Check</div>
						<div className="font-medium text-[var(--color-text)]">
							{formatNextCheckTime(
								nextCheckQuery.data?.data,
								taskStatus || undefined
							)}
						</div>
					</div>
					<button
						onClick={() => triggerCheckMutation.mutate()}
						disabled={isChecking}
						className="rounded-lg p-2 text-[var(--color-subtext)] transition-all hover:bg-[var(--color-bg-alt)] hover:text-[var(--color-text)] disabled:cursor-not-allowed disabled:opacity-50"
						title="Refresh Now"
					>
						<RefreshCw size={18} className={isChecking ? "animate-spin" : ""} />
					</button>
					<button
						onClick={() => setIsExpanded(!isExpanded)}
						className="rounded-lg p-2 text-[var(--color-subtext)] transition-all hover:bg-[var(--color-bg-alt)] hover:text-[var(--color-text)]"
						title={isExpanded ? "Collapse" : "Expand"}
					>
						<ChevronDown
							size={20}
							className={`transition-transform duration-300 ${isExpanded ? "rotate-180" : ""}`}
						/>
					</button>
				</div>
			</div>

			{/* Body - List of origins */}
			<div className="divide-y divide-[var(--color-bg-alt)] border-t border-[var(--color-bg-alt)]">
				{/* Always visible unhealthy items */}
				{unhealthy.map((item) => (
					<MonitorItem key={item.id} item={item} />
				))}

				{/* Collapsible section for healthy and pending items */}
				<AnimatePresence>
					{isExpanded && (
						<motion.div
							initial={{ height: 0, opacity: 0 }}
							animate={{ height: "auto", opacity: 1 }}
							exit={{ height: 0, opacity: 0 }}
							transition={{ duration: 0.3, ease: "easeInOut" }}
							className="overflow-hidden"
						>
							<div className="divide-y divide-[var(--color-bg-alt)]">
								{healthy.map((item) => (
									<MonitorItem key={item.id} item={item} />
								))}
								{pending.map((item) => (
									<MonitorItem key={item.id} item={item} />
								))}
							</div>
						</motion.div>
					)}
				</AnimatePresence>

				{/* Empty State */}
				{origins.length === 0 && (
					<div className="p-12 text-center text-[var(--color-subtext)]">
						<p className="font-medium">No origins to monitor.</p>
						<p className="text-sm">
							Add an origin in the card above to begin health monitoring.
						</p>
					</div>
				)}
			</div>
		</div>
	);
}
