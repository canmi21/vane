/* src/routes/$instance/origins/index.tsx */

import { createFileRoute, useParams } from "@tanstack/react-router";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Server, ServerCrash } from "lucide-react";
import React, { useMemo, useEffect } from "react";
import * as Tooltip from "@radix-ui/react-tooltip";
import { type RequestResult } from "~/api/request";
import {
	getInstance,
	postInstance,
	putInstance,
	deleteInstance,
} from "~/api/instance";
import { SummaryCard } from "~/components/origins/summary-card";
import { OriginListCard } from "~/components/origins/origin-list-card";
// --- NEW IMPORT ---
import { OriginMonitorCard } from "~/components/origins/origin-monitor-card";

// --- API Helper Functions for Origins ---
async function createOrigin(
	instanceId: string,
	url: string
): Promise<RequestResult<OriginResponse>> {
	return postInstance<OriginResponse>(instanceId, "/v1/origins", { url });
}

export type UpdateOriginPayload = {
	raw_url?: string;
	skip_ssl_verify?: boolean;
};

async function updateOrigin(
	instanceId: string,
	originId: string,
	payload: UpdateOriginPayload
): Promise<RequestResult<OriginResponse>> {
	return putInstance<OriginResponse>(
		instanceId,
		`/v1/origins/${originId}`,
		payload
	);
}

async function deleteOrigin(
	instanceId: string,
	originId: string
): Promise<RequestResult<unknown>> {
	return deleteInstance<unknown>(instanceId, `/v1/origins/${originId}`);
}

// --- NEW API Helper Functions for Monitor ---
async function getMonitorStatus(
	instanceId: string
): Promise<RequestResult<MonitorReportsStore>> {
	return getInstance(instanceId, "/v1/monitor/origins");
}

async function getTaskStatus(
	instanceId: string
): Promise<RequestResult<TaskStatus>> {
	return getInstance(instanceId, "/v1/monitor/origins/task-status");
}

async function getNextCheckTime(
	instanceId: string
): Promise<RequestResult<string | null>> {
	return getInstance(instanceId, "/v1/monitor/origins/next-check");
}

async function triggerCheckNow(
	instanceId: string
): Promise<RequestResult<unknown>> {
	return postInstance(instanceId, "/v1/monitor/origins/trigger-check", {});
}

// --- Data Types for Origins ---
export interface OriginResponse {
	id: string;
	scheme: "http" | "https";
	host: string;
	port: number;
	path: string;
	skip_ssl_verify: boolean;
	raw_url: string;
}

// --- NEW Data Types for Monitor ---
export type OriginStatus = "healthy" | "unhealthy" | "pending";
export type TaskStatus = "idle" | "running";

export interface OriginMonitorReport {
	status: OriginStatus;
	check_url: string;
	last_checked: string; // ISO 8601 date string
	last_message: string;
}

export type MonitorReportsStore = Record<string, OriginMonitorReport>;

// --- Route Definition ---
export const Route = createFileRoute("/$instance/origins/")({
	component: OriginsPage,
});

// --- Main Page Component ---
function OriginsPage() {
	const { instance: instanceId } = useParams({ from: "/$instance/origins/" });
	const queryClient = useQueryClient();

	// --- Query for Origins List ---
	const {
		data: originsResult,
		isLoading,
		isError,
		error,
	} = useQuery<RequestResult<OriginResponse[]>>({
		queryKey: ["instance", instanceId, "origins"],
		queryFn: () => getInstance(instanceId, "/v1/origins"),
	});

	// --- NEW Queries for Health Monitor ---
	// This query will be refetched on a fixed interval.
	const monitorStatusQuery = useQuery<RequestResult<MonitorReportsStore>>({
		queryKey: ["instance", instanceId, "monitor", "status"],
		queryFn: () => getMonitorStatus(instanceId),
		refetchInterval: 5000, // Poll every 5 seconds
		refetchOnWindowFocus: true,
	});

	// The task status also polls to update UI state (e.g., spinning icon).
	const taskStatusQuery = useQuery<RequestResult<TaskStatus>>({
		queryKey: ["instance", instanceId, "monitor", "taskStatus"],
		queryFn: () => getTaskStatus(instanceId),
		refetchInterval: 2000, // Poll more frequently for responsiveness
	});

	// The next check time also polls.
	const nextCheckQuery = useQuery<RequestResult<string | null>>({
		queryKey: ["instance", instanceId, "monitor", "nextCheck"],
		queryFn: () => getNextCheckTime(instanceId),
		refetchInterval: 2000,
	});

	const origins = useMemo(
		() => originsResult?.data ?? [],
		[originsResult?.data]
	);

	// Memoize monitor reports to avoid re-renders.
	const monitorReports = useMemo(
		() => monitorStatusQuery.data?.data ?? {},
		[monitorStatusQuery.data?.data]
	);

	const stats = useMemo(() => {
		const httpCount = origins.filter((o) => o.scheme === "http").length;
		const httpsCount = origins.filter((o) => o.scheme === "https").length;
		const sslSkipped = origins.filter((o) => o.skip_ssl_verify).length;
		return {
			total: origins.length,
			http: httpCount,
			https: httpsCount,
			sslSkipped,
		};
	}, [origins]);

	// --- Mutations for Origins ---
	const addMutation = useMutation<RequestResult<OriginResponse>, Error, string>(
		{
			mutationFn: (newUrl) => createOrigin(instanceId, newUrl),
			onSuccess: () => {
				queryClient.invalidateQueries({
					queryKey: ["instance", instanceId, "origins"],
				});
			},
		}
	);

	const updateMutation = useMutation<
		RequestResult<OriginResponse>,
		Error,
		{ id: string; payload: UpdateOriginPayload }
	>({
		mutationFn: (variables) =>
			updateOrigin(instanceId, variables.id, variables.payload),
		onSuccess: () => {
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "origins"],
			});
		},
	});

	const removeMutation = useMutation<RequestResult<unknown>, Error, string>({
		mutationFn: (id) => deleteOrigin(instanceId, id),
		onSuccess: () => {
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "origins"],
			});
		},
	});

	// --- NEW Mutation for Triggering a Check ---
	const triggerCheckMutation = useMutation<RequestResult<unknown>, Error, void>(
		{
			mutationFn: () => triggerCheckNow(instanceId),
			onSuccess: () => {
				// After triggering, immediately refetch all monitor data.
				queryClient.invalidateQueries({
					queryKey: ["instance", instanceId, "monitor"],
				});
			},
		}
	);

	// When origins list changes, invalidate monitor status to sync up.
	useEffect(() => {
		if (!isLoading) {
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "monitor", "status"],
			});
		}
	}, [origins.length, isLoading, instanceId, queryClient]);

	if (isLoading) {
		return <StatusCard icon={Server} text="Loading Origins..." />;
	}
	if (isError) {
		return (
			<StatusCard
				icon={ServerCrash}
				text={error?.message || "Failed to fetch origins."}
				isError
			/>
		);
	}

	return (
		<Tooltip.Provider delayDuration={200}>
			<div className="space-y-6">
				<SummaryCard stats={stats} />
				<OriginListCard
					origins={origins}
					addMutation={addMutation}
					updateMutation={updateMutation}
					removeMutation={removeMutation}
				/>
				{/* --- NEW CARD RENDER --- */}
				<OriginMonitorCard
					origins={origins}
					monitorReports={monitorReports}
					taskStatusQuery={taskStatusQuery}
					nextCheckQuery={nextCheckQuery}
					triggerCheckMutation={triggerCheckMutation}
				/>
			</div>
		</Tooltip.Provider>
	);
}

// --- StatusCard Component (unchanged) ---
function StatusCard({
	icon: Icon,
	text,
	isError = false,
}: {
	icon: React.ElementType;
	text: string;
	isError?: boolean;
}) {
	const colorClass = isError ? "text-red-500" : "text-[var(--color-subtext)]";
	return (
		<div className="flex w-full items-center justify-center rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] p-12 shadow-sm">
			<div className="flex flex-col items-center gap-4">
				<Icon size={32} className={colorClass} />
				<p className={`text-center font-medium ${colorClass}`}>{text}</p>
			</div>
		</div>
	);
}
