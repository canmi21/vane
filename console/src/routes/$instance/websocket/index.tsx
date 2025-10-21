/* src/routes/$instance/websocket/index.tsx */

import {
	createFileRoute,
	useParams,
	useNavigate,
	useLocation,
} from "@tanstack/react-router";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Server, ServerCrash } from "lucide-react";
import React, { useState, useEffect, useMemo, useCallback } from "react";
import { type RequestResult } from "~/api/request";
import { getInstance, putInstance, deleteInstance } from "~/api/instance";
import { WebSocketListCard } from "~/components/websocket/websocket-list-card";
import { WebSocketEditorCard } from "~/components/websocket/websocket-editor-card";
import * as Tooltip from "@radix-ui/react-tooltip";

// --- API Helper Functions ---
async function listDomains(
	instanceId: string
): Promise<RequestResult<{ domains: string[] }>> {
	return getInstance(instanceId, "/v1/domains");
}

async function getWebSocketConfig(
	instanceId: string,
	domain: string
): Promise<RequestResult<WebSocketConfig>> {
	return getInstance(instanceId, `/v1/websocket/${domain}`);
}

async function updateWebSocketConfig(
	instanceId: string,
	domain: string,
	config: WebSocketConfig
): Promise<RequestResult<WebSocketConfig>> {
	return putInstance(instanceId, `/v1/websocket/${domain}`, config as never);
}

async function resetWebSocketConfig(
	instanceId: string,
	domain: string
): Promise<RequestResult<unknown>> {
	return deleteInstance(instanceId, `/v1/websocket/${domain}`);
}

// --- Data Types from Backend ---
export interface WebSocketConfig {
	enabled: boolean;
	paths: string[]; // MODIFIED: Changed from 'path' to 'paths' array
}

export const Route = createFileRoute("/$instance/websocket/")({
	component: WebSocketPage,
});

function WebSocketPage() {
	const { instance: instanceId } = useParams({ from: "/$instance/websocket/" });
	const queryClient = useQueryClient();
	const navigate = useNavigate();
	const location = useLocation();

	const [selectedDomain, setSelectedDomain] = useState<string | null>(null);

	// --- Step 1: Query for the list of all configured domains ---
	const {
		data: domainsResult,
		isLoading,
		isError,
		error,
	} = useQuery<RequestResult<{ domains: string[] }>>({
		queryKey: ["instance", instanceId, "domains"],
		queryFn: () => listDomains(instanceId),
	});

	const domains = useMemo(
		() => [...(domainsResult?.data?.domains ?? []), "fallback"],
		[domainsResult]
	);

	// --- Step 2: Query for the selected domain's WebSocket config ---
	const selectedConfigQuery = useQuery<RequestResult<WebSocketConfig>>({
		queryKey: ["instance", instanceId, "websocket", selectedDomain],
		queryFn: () => getWebSocketConfig(instanceId, selectedDomain!),
		enabled: !!selectedDomain,
	});

	// --- Handler for selection and URL sync ---
	const handleDomainSelect = useCallback(
		(domain: string | null) => {
			setSelectedDomain(domain);
			navigate({
				hash: domain ? encodeURIComponent(domain) : "",
				replace: true,
			});
		},
		[navigate]
	);

	// --- Logic to sync state from URL on load ---
	useEffect(() => {
		if (isLoading) return;
		const hashDomain = location.hash
			? decodeURIComponent(location.hash.slice(1))
			: null;

		if (hashDomain && domains.includes(hashDomain)) {
			if (selectedDomain !== hashDomain) setSelectedDomain(hashDomain);
			return;
		}
		if (!selectedDomain || !domains.includes(selectedDomain)) {
			if (domains.length > 0) handleDomainSelect(domains[0]);
			else handleDomainSelect(null);
		}
	}, [domains, isLoading, location.hash, selectedDomain, handleDomainSelect]);

	// --- Mutations for WebSocket management ---
	const updateMutation = useMutation<
		RequestResult<WebSocketConfig>,
		Error,
		{ domain: string; config: WebSocketConfig }
	>({
		mutationFn: (vars) =>
			updateWebSocketConfig(instanceId, vars.domain, vars.config),
		onSuccess: (_, vars) => {
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "websocket", vars.domain],
			});
		},
	});

	const resetMutation = useMutation<RequestResult<unknown>, Error, string>({
		mutationFn: (domain) => resetWebSocketConfig(instanceId, domain),
		onSuccess: (_, domain) => {
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "websocket", domain],
			});
		},
	});

	if (isLoading) {
		return <StatusCard icon={Server} text="Loading Domains..." />;
	}
	if (isError) {
		return (
			<StatusCard
				icon={ServerCrash}
				text={error?.message || "Failed to fetch domains."}
				isError
			/>
		);
	}

	return (
		<Tooltip.Provider delayDuration={200}>
			<div className="space-y-6">
				<WebSocketListCard
					domains={domains}
					selectedDomain={selectedDomain}
					onSelectDomain={handleDomainSelect}
				/>
				{selectedDomain && (
					<WebSocketEditorCard
						key={selectedDomain}
						domain={selectedDomain}
						query={selectedConfigQuery}
						updateMutation={updateMutation}
						resetMutation={resetMutation}
					/>
				)}
			</div>
		</Tooltip.Provider>
	);
}

// --- StatusCard Component ---
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
