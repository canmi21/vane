/* src/routes/$instance/websocket/index.tsx */

import {
	createFileRoute,
	useParams,
	useNavigate,
	useLocation,
} from "@tanstack/react-router";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Server, ServerCrash, Cable, PlugZap } from "lucide-react";
import React, { useState, useEffect, useMemo, useCallback } from "react";
import { type RequestResult } from "~/api/request";
import { getInstance, putInstance, deleteInstance } from "~/api/instance";
import {
	DomainListCard,
	type DomainListItem,
} from "~/components/shared/domain-list-card";
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
	paths: string[];
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

	const {
		data: domainsResult,
		isLoading: isListLoading,
		isError: isListError,
		error: listError,
	} = useQuery<RequestResult<{ domains: string[] }>>({
		queryKey: ["instance", instanceId, "domains"],
		queryFn: () => listDomains(instanceId),
	});
	const domains = useMemo(
		() => [...(domainsResult?.data?.domains ?? []), "fallback"],
		[domainsResult]
	);

	const { data: allConfigs, isLoading: areConfigsLoading } = useQuery<
		Record<string, WebSocketConfig>
	>({
		queryKey: ["instance", instanceId, "websocket", "all"],
		queryFn: async () => {
			const promises = domains.map(async (domain) => {
				const res = await getWebSocketConfig(instanceId, domain);
				return { domain, config: res.data };
			});
			const results = await Promise.all(promises);
			return results.reduce(
				(acc, { domain, config }) => {
					if (config) acc[domain] = config;
					return acc;
				},
				{} as Record<string, WebSocketConfig>
			);
		},
		enabled: domains.length > 0,
	});

	const domainListItems = useMemo<DomainListItem[]>(() => {
		return domains.map((domain) => {
			const isEnabled = allConfigs?.[domain]?.enabled ?? false;
			return {
				domain,
				badge: {
					icon: isEnabled ? Cable : PlugZap,
					text: isEnabled ? "Enabled" : "Disabled",
				},
			};
		});
	}, [domains, allConfigs]);

	const selectedConfigQuery = useQuery<RequestResult<WebSocketConfig>>({
		queryKey: ["instance", instanceId, "websocket", selectedDomain],
		queryFn: () => getWebSocketConfig(instanceId, selectedDomain!),
		enabled: !!selectedDomain,
	});

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
	useEffect(() => {
		if (isListLoading) return;
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
	}, [
		domains,
		isListLoading,
		location.hash,
		selectedDomain,
		handleDomainSelect,
	]);

	const updateMutation = useMutation<
		RequestResult<WebSocketConfig>,
		Error,
		{ domain: string; config: WebSocketConfig }
	>({
		mutationFn: (vars) =>
			updateWebSocketConfig(instanceId, vars.domain, vars.config),
		onSuccess: () =>
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "websocket"],
			}),
	});
	const resetMutation = useMutation<RequestResult<unknown>, Error, string>({
		mutationFn: (domain) => resetWebSocketConfig(instanceId, domain),
		onSuccess: () =>
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "websocket"],
			}),
	});

	const isLoading = isListLoading || (domains.length > 0 && areConfigsLoading);
	const isError = isListError;
	const error = listError;

	if (isLoading) return <StatusCard icon={Server} text="Loading Domains..." />;
	if (isError)
		return (
			<StatusCard
				icon={ServerCrash}
				text={error?.message || "Failed to fetch domains."}
				isError
			/>
		);

	return (
		<Tooltip.Provider delayDuration={200}>
			<div className="space-y-6">
				<DomainListCard
					title="WebSocket Proxy Policies"
					icon={Cable}
					items={domainListItems}
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
