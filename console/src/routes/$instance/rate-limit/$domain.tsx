/* src/routes/$instance/rate-limit/$domain.tsx */

import {
	createFileRoute,
	useParams,
	useNavigate,
} from "@tanstack/react-router";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Server, ServerCrash, Gauge, ZapOff } from "lucide-react";
import React, { useMemo, useCallback } from "react";
import { type RequestResult } from "~/api/request";
import { getInstance, putInstance, deleteInstance } from "~/api/instance";
import {
	DomainListCard,
	type DomainListItem,
} from "~/components/shared/domain-list-card";
import { RateLimitEditorCard } from "~/components/rate-limit/rate-limit-editor-card";
import * as Tooltip from "@radix-ui/react-tooltip";

// --- API Helper Functions ---
async function listDomains(
	instanceId: string
): Promise<RequestResult<{ domains: string[] }>> {
	return getInstance(instanceId, "/v1/domains");
}
async function getRateLimitConfig(
	instanceId: string,
	domain: string
): Promise<RequestResult<RateLimitConfig>> {
	return getInstance(instanceId, `/v1/ratelimit/${domain}`);
}
async function updateRateLimitConfig(
	instanceId: string,
	domain: string,
	config: RateLimitConfig
): Promise<RequestResult<RateLimitConfig>> {
	return putInstance(instanceId, `/v1/ratelimit/${domain}`, config as never);
}
async function resetRateLimitConfig(
	instanceId: string,
	domain: string
): Promise<RequestResult<unknown>> {
	return deleteInstance(instanceId, `/v1/ratelimit/${domain}`);
}

// --- Data Types from Backend ---
export interface RateLimitConfig {
	requests_per_second: number;
}

export const Route = createFileRoute("/$instance/rate-limit/$domain")({
	component: RateLimitDetailPage,
});

function RateLimitDetailPage() {
	const { instance: instanceId, domain } = useParams({
		from: "/$instance/rate-limit/$domain",
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
		const apiDomains = domainsResult?.data?.domains ?? [];
		const sortedApiDomains = apiDomains.sort((a, b) => {
			const aIsWildcard = a.includes("*");
			const bIsWildcard = b.includes("*");
			if (aIsWildcard !== bIsWildcard) {
				return aIsWildcard ? 1 : -1;
			}
			return a.localeCompare(b);
		});
		return [...sortedApiDomains, "fallback"];
	}, [domainsResult]);

	const { data: allConfigs, isLoading: areConfigsLoading } = useQuery<
		Record<string, RateLimitConfig>
	>({
		queryKey: ["instance", instanceId, "ratelimit", "all"],
		queryFn: async () => {
			const promises = domains.map(async (domain) => {
				const res = await getRateLimitConfig(instanceId, domain);
				return { domain, config: res.data };
			});
			const results = await Promise.all(promises);
			return results.reduce(
				(acc, { domain, config }) => {
					if (config) acc[domain] = config;
					return acc;
				},
				{} as Record<string, RateLimitConfig>
			);
		},
		enabled: domains.length > 0,
	});

	const domainListItems = useMemo<DomainListItem[]>(() => {
		return domains.map((domain) => {
			const rps = allConfigs?.[domain]?.requests_per_second ?? 0;
			return {
				domain,
				badge: {
					icon: rps > 0 ? Gauge : ZapOff,
					text: rps > 0 ? `${rps} req/s` : "Disabled",
				},
			};
		});
	}, [domains, allConfigs]);

	const selectedConfigQuery = useQuery<RequestResult<RateLimitConfig>>({
		queryKey: ["instance", instanceId, "ratelimit", selectedDomain],
		queryFn: () => getRateLimitConfig(instanceId, selectedDomain!),
		enabled: !!selectedDomain,
	});

	const handleDomainSelect = useCallback(
		(newDomain: string | null) => {
			navigate({
				to: "/$instance/rate-limit/$domain",
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
		RequestResult<RateLimitConfig>,
		Error,
		{ domain: string; config: RateLimitConfig }
	>({
		mutationFn: (vars) =>
			updateRateLimitConfig(instanceId, vars.domain, vars.config),
		onSuccess: () =>
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "ratelimit"],
			}),
	});
	const resetMutation = useMutation<RequestResult<unknown>, Error, string>({
		mutationFn: (domain) => resetRateLimitConfig(instanceId, domain),
		onSuccess: (_, deletedDomain) => {
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "ratelimit"],
			});
			if (selectedDomain === deletedDomain) {
				handleDomainSelect(null);
			}
		},
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
					title="Rate Limiting Policies"
					icon={Gauge}
					items={domainListItems}
					selectedDomain={selectedDomain}
					onSelectDomain={handleDomainSelect}
				/>
				{selectedDomain && (
					<RateLimitEditorCard
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
