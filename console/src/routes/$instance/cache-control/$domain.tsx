/* src/routes/$instance/cache-control/$domain.tsx */

import {
	createFileRoute,
	useParams,
	useNavigate,
} from "@tanstack/react-router";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Server, ServerCrash, History } from "lucide-react";
import React, { useMemo, useCallback } from "react";
import { type RequestResult } from "~/api/request";
import { getInstance, putInstance, deleteInstance } from "~/api/instance";
import {
	DomainListCard,
	type DomainListItem,
} from "~/components/shared/domain-list-card";
import { CacheEditorCard } from "~/components/cache-control/cache-editor-card";
import * as Tooltip from "@radix-ui/react-tooltip";

// --- API Helper Functions ---
async function listDomains(
	instanceId: string
): Promise<RequestResult<{ domains: string[] }>> {
	return getInstance(instanceId, "/v1/domains");
}
async function getCacheConfig(
	instanceId: string,
	domain: string
): Promise<RequestResult<CacheConfig>> {
	return getInstance(instanceId, `/v1/cache/${domain}`);
}
async function updateCacheConfig(
	instanceId: string,
	domain: string,
	config: CacheConfig
): Promise<RequestResult<CacheConfig>> {
	return putInstance(instanceId, `/v1/cache/${domain}`, config as never);
}
async function resetCacheConfig(
	instanceId: string,
	domain: string
): Promise<RequestResult<unknown>> {
	return deleteInstance(instanceId, `/v1/cache/${domain}`);
}

// --- Data Types from Backend ---
export interface CacheRule {
	path: string;
	ttl_seconds: number;
}
export interface CacheConfig {
	respect_origin_cache_control: boolean;
	path_rules: CacheRule[];
	blacklist_paths: string[];
}

export const Route = createFileRoute("/$instance/cache-control/$domain")({
	component: CacheControlDetailPage,
});

function CacheControlDetailPage() {
	const { instance: instanceId, domain } = useParams({
		from: "/$instance/cache-control/$domain",
	});
	const selectedDomain = domain === "_" ? null : domain;

	const queryClient = useQueryClient();
	const navigate = useNavigate();

	const {
		data: domainsResult,
		isLoading: isListLoading,
		isError: isListError,
		error: listError,
	} = useQuery<RequestResult<{ domains: string[] }>>({
		queryKey: ["instance", instanceId, "domains"],
		queryFn: () => listDomains(instanceId),
	});

	const domains = useMemo(() => {
		// 1. Get the domains from the API.
		const apiDomains = domainsResult?.data?.domains ?? [];

		// 2. Sort only the API domains.
		const sortedApiDomains = apiDomains.sort((a, b) => {
			const aIsWildcard = a.includes("*");
			const bIsWildcard = b.includes("*");
			// Push wildcard domains to the end of this list.
			if (aIsWildcard !== bIsWildcard) {
				return aIsWildcard ? 1 : -1;
			}
			// Otherwise, sort alphabetically.
			return a.localeCompare(b);
		});

		// 3. Append the special 'fallback' domain at the very end.
		return [...sortedApiDomains, "fallback"];
	}, [domainsResult]);

	const { data: allConfigs, isLoading: areConfigsLoading } = useQuery<
		Record<string, CacheConfig>
	>({
		queryKey: ["instance", instanceId, "cache", "all"],
		queryFn: async () => {
			const promises = domains.map(async (domain) => {
				const res = await getCacheConfig(instanceId, domain);
				return { domain, config: res.data };
			});
			const results = await Promise.all(promises);
			return results.reduce(
				(acc, { domain, config }) => {
					if (config) acc[domain] = config;
					return acc;
				},
				{} as Record<string, CacheConfig>
			);
		},
		enabled: domains.length > 0,
	});

	const domainListItems = useMemo<DomainListItem[]>(() => {
		return domains.map((domain) => {
			const config = allConfigs?.[domain];
			const ruleCount = config?.path_rules.length ?? 0;
			return {
				domain,
				badge: {
					icon: History,
					text: `${ruleCount} Rule(s)`,
				},
			};
		});
	}, [domains, allConfigs]);

	const selectedConfigQuery = useQuery<RequestResult<CacheConfig>>({
		queryKey: ["instance", instanceId, "cache", selectedDomain],
		queryFn: () => getCacheConfig(instanceId, selectedDomain!),
		enabled: !!selectedDomain,
	});

	const handleDomainSelect = useCallback(
		(newDomain: string | null) => {
			navigate({
				to: "/$instance/cache-control/$domain",
				params: {
					instance: instanceId,
					domain: newDomain || "_",
				},
				replace: true,
			});
		},
		[navigate, instanceId]
	);

	const updateMutation = useMutation<
		RequestResult<CacheConfig>,
		Error,
		{ domain: string; config: CacheConfig }
	>({
		mutationFn: (vars) =>
			updateCacheConfig(instanceId, vars.domain, vars.config),
		onSuccess: () =>
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "cache"],
			}),
	});
	const resetMutation = useMutation<RequestResult<unknown>, Error, string>({
		mutationFn: (domain) => resetCacheConfig(instanceId, domain),
		onSuccess: () =>
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "cache"],
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
					title="Cache Control Policies"
					icon={History}
					items={domainListItems}
					selectedDomain={selectedDomain}
					onSelectDomain={handleDomainSelect}
				/>
				{selectedDomain && (
					<CacheEditorCard
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
