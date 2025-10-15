/* src/components/origins/monitor-item.tsx */

import {
	CheckCircle2,
	XCircle,
	HelpCircle,
	Info,
	ExternalLink,
} from "lucide-react";
import * as Tooltip from "@radix-ui/react-tooltip";
import { type OriginResponse } from "~/routes/$instance/origins/";
import { type OriginMonitorReport } from "~/routes/$instance/origins/";

// A type that merges origin data with its monitor report.
export type MonitoredOrigin = OriginResponse & {
	report?: OriginMonitorReport;
};

// Helper to format date strings.
function formatDateTime(dateString?: string): string {
	if (!dateString) return "N/A";
	try {
		return new Intl.DateTimeFormat("en-US", {
			year: "numeric",
			month: "short",
			day: "numeric",
			hour: "2-digit",
			minute: "2-digit",
			second: "2-digit",
			hour12: false,
		}).format(new Date(dateString));
	} catch {
		return "Invalid Date";
	}
}

export function MonitorItem({ item }: { item: MonitoredOrigin }) {
	const status = item.report?.status ?? "pending";
	const isHealthy = status === "healthy";
	const colorClass = isHealthy
		? "text-[var(--color-text)]"
		: "text-[var(--color-subtext)]";
	const iconColorClass = isHealthy
		? "stroke-[var(--color-text)]"
		: "stroke-[var(--color-subtext)]";

	const StatusIcon = () => {
		switch (status) {
			case "healthy":
				return <CheckCircle2 size={16} className={iconColorClass} />;
			case "unhealthy":
				return <XCircle size={16} className={iconColorClass} />;
			default:
				return <HelpCircle size={16} className={iconColorClass} />;
		}
	};

	return (
		<div className={`px-4 py-2.5 ${colorClass}`}>
			<div className="flex items-center gap-3">
				{/* Status Icon */}
				<div className="flex-shrink-0">
					<StatusIcon />
				</div>

				{/* URL and basic info */}
				<div className="flex min-w-0 flex-grow items-center gap-3">
					<span className="truncate font-mono text-sm font-medium">
						{item.raw_url}
					</span>
					<span className="flex-shrink-0 rounded-md bg-[var(--color-bg-alt)] px-2 py-0.5 font-mono text-xs text-[var(--color-subtext)]">
						{item.id}
					</span>
				</div>

				{/* Status Text and Info Tooltip */}
				<div className="flex flex-shrink-0 items-center gap-2">
					<span className="text-xs font-medium capitalize">{status}</span>
					<Tooltip.Root>
						<Tooltip.Trigger asChild>
							<button className="rounded-md p-1 text-[var(--color-subtext)] transition-all hover:scale-110 hover:text-[var(--color-theme-border)]">
								<Info size={14} />
							</button>
						</Tooltip.Trigger>
						<Tooltip.Portal>
							<Tooltip.Content
								className="z-50 w-72 rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg)] p-3 shadow-lg"
								sideOffset={5}
							>
								<div className="space-y-2 text-xs">
									<p className="font-bold text-[var(--color-text)]">
										Health Check Details
									</p>
									<div className="flex justify-between gap-4">
										<span className="text-[var(--color-subtext)]">
											Check URL:
										</span>
										<a
											href={item.report?.check_url}
											target="_blank"
											rel="noopener noreferrer"
											className="group flex items-center gap-1 truncate font-mono font-medium text-[var(--color-text)] hover:underline"
										>
											<span className="truncate">
												{item.report?.check_url ?? "N/A"}
											</span>
											<ExternalLink
												size={12}
												className="opacity-0 transition-opacity group-hover:opacity-100"
											/>
										</a>
									</div>
									<div className="flex justify-between gap-4">
										<span className="text-[var(--color-subtext)]">
											Last Checked:
										</span>
										<span className="font-mono font-medium text-[var(--color-text)]">
											{formatDateTime(item.report?.last_checked)}
										</span>
									</div>
									<div className="flex flex-col gap-1">
										<span className="text-[var(--color-subtext)]">
											Last Message:
										</span>
										<p className="break-words font-mono font-medium text-[var(--color-text)]">
											{item.report?.last_message ?? "No check performed yet."}
										</p>
									</div>
								</div>
								<Tooltip.Arrow className="fill-[var(--color-bg-alt)]" />
							</Tooltip.Content>
						</Tooltip.Portal>
					</Tooltip.Root>
				</div>
			</div>
		</div>
	);
}
