/* src/components/origins/monitor-item.tsx */

import {
	CheckCircle2,
	XCircle,
	HelpCircle,
	Info,
	ExternalLink,
	Pencil,
	Save,
	X,
	History,
} from "lucide-react";
import { useState } from "react";
import * as Tooltip from "@radix-ui/react-tooltip";
import { type UseMutationResult } from "@tanstack/react-query";
import { type RequestResult } from "~/api/request";
import {
	type OriginResponse,
	type OriginMonitorReport,
	type MonitorConfig,
} from "~/routes/$instance/origins/";

// A type that merges origin data with its monitor report.
export type MonitoredOrigin = OriginResponse & {
	report?: OriginMonitorReport;
};

// Helper to format date strings.
function formatDateTime(dateString?: string): string {
	if (!dateString) return "NA";
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

export function MonitorItem({
	item,
	setOverrideMutation,
	deleteOverrideMutation,
}: {
	item: MonitoredOrigin;
	setOverrideMutation: UseMutationResult<
		RequestResult<MonitorConfig>,
		Error,
		{ originId: string; url: string }
	>;
	deleteOverrideMutation: UseMutationResult<
		RequestResult<MonitorConfig>,
		Error,
		string
	>;
}) {
	const [isEditing, setIsEditing] = useState(false);
	const [overrideUrl, setOverrideUrl] = useState(item.report?.check_url || "");

	const defaultCheckUrl = `${item.scheme}://${item.host}:${item.port}${item.path === "/" ? "" : item.path}`;
	const isOverridden = item.report?.check_url !== defaultCheckUrl;

	const handleEdit = () => {
		setOverrideUrl(item.report?.check_url || defaultCheckUrl);
		setIsEditing(true);
	};

	const handleCancel = () => {
		setIsEditing(false);
	};

	const handleSave = () => {
		const trimmedUrl = overrideUrl.trim();
		if (
			!trimmedUrl.startsWith("http://") &&
			!trimmedUrl.startsWith("https://")
		) {
			alert("Invalid URL. Must start with 'http://' or 'https://'.");
			return;
		}
		if (trimmedUrl === item.report?.check_url) {
			setIsEditing(false); // No changes made
			return;
		}
		setOverrideMutation.mutate(
			{ originId: item.id, url: trimmedUrl },
			{ onSuccess: () => setIsEditing(false) }
		);
	};

	const handleReset = () => {
		if (
			window.confirm(
				"Are you sure you want to reset the check URL to its default?"
			)
		) {
			deleteOverrideMutation.mutate(item.id, {
				onSuccess: () => setIsEditing(false),
			});
		}
	};

	const isMutating =
		setOverrideMutation.isPending || deleteOverrideMutation.isPending;
	const mutationError =
		setOverrideMutation.error || deleteOverrideMutation.error;

	// Display logic
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

	if (isEditing) {
		return (
			<div className="bg-[var(--color-theme-bg)] p-3">
				<div className="flex items-center gap-2">
					<input
						type="text"
						value={overrideUrl}
						onChange={(e) => setOverrideUrl(e.target.value)}
						className="flex-grow rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg-alt)] px-3 py-1.5 font-mono text-sm text-[var(--color-text)] placeholder-[var(--color-subtext)] transition-all focus:border-[var(--color-theme-border)] focus:outline-none focus:ring-1 focus:ring-[var(--color-theme-border)]"
						disabled={isMutating}
					/>
					{isOverridden && (
						<button
							onClick={handleReset}
							disabled={isMutating}
							className="rounded-md p-1.5 text-[var(--color-subtext)] hover:text-[var(--color-text)] disabled:opacity-50"
							title="Reset to Default URL"
						>
							<History size={18} />
						</button>
					)}
					<button
						onClick={handleSave}
						disabled={isMutating}
						className="rounded-md p-1.5 text-[var(--color-subtext)] hover:text-[var(--color-theme-border)] disabled:opacity-50"
						title="Save Changes"
					>
						<Save size={18} />
					</button>
					<button
						onClick={handleCancel}
						disabled={isMutating}
						className="rounded-md p-1.5 text-[var(--color-subtext)] hover:text-[var(--color-text)]"
						title="Cancel"
					>
						<X size={18} />
					</button>
				</div>
				{mutationError && (
					<p className="mt-2 text-xs text-red-500">
						{mutationError.message || "An error occurred."}
					</p>
				)}
			</div>
		);
	}

	return (
		<div
			className={`px-4 py-2.5 transition-all hover:bg-[var(--color-theme-bg)] ${colorClass}`}
		>
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

				{/* Status Text */}
				<div className="flex flex-shrink-0 items-center gap-2">
					<span className="text-xs font-medium capitalize">{status}</span>
				</div>

				{/* Action Buttons */}
				<div className="flex flex-shrink-0 items-center gap-1">
					<Tooltip.Root>
						<Tooltip.Trigger asChild>
							<button className="rounded-md p-1.5 text-[var(--color-subtext)] transition-all hover:scale-110 hover:text-[var(--color-theme-border)]">
								<Info size={16} />
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
									{/* --- FIX APPLIED HERE: Changed flex-col to flex justify-between --- */}
									<div className="flex justify-between gap-4">
										<span className="flex-shrink-0 text-[var(--color-subtext)]">
											Last Message:
										</span>
										<span className="break-words text-right font-mono font-medium text-[var(--color-text)]">
											{item.report?.last_message ?? "No check performed yet."}
										</span>
									</div>
								</div>
								<Tooltip.Arrow className="fill-[var(--color-bg-alt)]" />
							</Tooltip.Content>
						</Tooltip.Portal>
					</Tooltip.Root>
					<button
						onClick={handleEdit}
						className="rounded-md p-1.5 text-[var(--color-subtext)] transition-all hover:scale-110 hover:text-[var(--color-theme-border)]"
						title="Edit Check URL"
					>
						<Pencil size={16} />
					</button>
				</div>
			</div>
		</div>
	);
}
