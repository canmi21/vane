/* src/routes/$instance/origins/index.tsx */

import { createFileRoute, useParams } from "@tanstack/react-router";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Server, ServerCrash } from "lucide-react";
import React, { useMemo } from "react";
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

// --- API Helper Functions ---
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

// --- Data Types ---
export interface OriginResponse {
	id: string;
	scheme: "http" | "https";
	host: string;
	port: number;
	path: string;
	skip_ssl_verify: boolean;
	raw_url: string;
}

export const Route = createFileRoute("/$instance/origins/")({
	component: OriginsPage,
});

// --- Main Page Component ---
function OriginsPage() {
	const { instance: instanceId } = useParams({ from: "/$instance/origins/" });
	const queryClient = useQueryClient();

	const {
		data: originsResult,
		isLoading,
		isError,
		error,
	} = useQuery<RequestResult<OriginResponse[]>>({
		queryKey: ["instance", instanceId, "origins"],
		queryFn: () => getInstance(instanceId, "/v1/origins"),
	});

	const origins = useMemo(
		() => originsResult?.data ?? [],
		[originsResult?.data]
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
			</div>
		</Tooltip.Provider>
	);
}

// --- StatusCard ---
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
