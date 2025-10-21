/* src/routes/$instance/cors-management/$domain.tsx */

import {
	createFileRoute,
	useParams,
	useNavigate,
} from "@tanstack/react-router";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import {
	Server,
	ServerCrash,
	ArrowRightLeft,
	Route as RouteIcon,
	RouteOff,
} from "lucide-react";
import React, { useMemo, useCallback } from "react";
import { type RequestResult } from "~/api/request";
import { getInstance, putInstance, deleteInstance } from "~/api/instance";
import {
	CorsOverviewCard,
	type CorsOverviewStats,
} from "~/components/cors/cors-overview-card";
import {
	DomainListCard,
	type DomainListItem,
} from "~/components/shared/domain-list-card";
import { CorsEditorCard } from "~/components/cors/cors-editor-card";
import * as Tooltip from "@radix-ui/react-tooltip";

// --- API Helper Functions ---
async function listCorsStatus(
	instanceId: string
): Promise<RequestResult<CorsStatus[]>> {
	return getInstance(instanceId, "/v1/cors");
}
async function getCorsConfig(
	instanceId: string,
	domain: string
): Promise<RequestResult<CorsConfig>> {
	return getInstance(instanceId, `/v1/cors/${domain}`);
}
async function updateCorsConfig(
	instanceId: string,
	domain: string,
	config: CorsConfig
): Promise<RequestResult<CorsConfig>> {
	return putInstance(instanceId, `/v1/cors/${domain}`, config as never);
}
async function resetCorsConfig(
	instanceId: string,
	domain: string
): Promise<RequestResult<unknown>> {
	return deleteInstance(instanceId, `/v1/cors/${domain}`);
}

// --- Data Types from Backend ---
export type PreflightHandling = "proxy_decision" | "origin_response";
export interface CorsStatus {
	domain: string;
	preflight_handling: PreflightHandling;
}
export interface CorsConfig {
	preflight_handling: PreflightHandling;
	allow_origins: string[];
	allow_methods: string[];
	allow_headers: string[];
	allow_credentials: boolean;
	expose_headers: string[];
	max_age_seconds: number;
}

export const Route = createFileRoute("/$instance/cors-management/$domain")({
	component: CorsManagementDetailPage,
});

function CorsManagementDetailPage() {
	const { instance: instanceId, domain } = useParams({
		from: "/$instance/cors-management/$domain",
	});
	const selectedDomain = domain === "_" ? null : domain;

	const queryClient = useQueryClient();
	const navigate = useNavigate();

	const {
		data: statusResult,
		isLoading: isListLoading,
		isError: isListError,
		error: listError,
	} = useQuery<RequestResult<CorsStatus[]>>({
		queryKey: ["instance", instanceId, "cors", "status"],
		queryFn: () => listCorsStatus(instanceId),
	});

	const corsStatuses = useMemo(() => {
		const statuses = statusResult?.data ?? [];
		// --- FIX: Apply the universal robust sorting logic ---
		return statuses.sort((a, b) => {
			const domainA = a.domain;
			const domainB = b.domain;

			// Rule 1: 'fallback' is always last.
			const aIsFallback = domainA === "fallback";
			const bIsFallback = domainB === "fallback";
			if (aIsFallback !== bIsFallback) {
				return aIsFallback ? 1 : -1;
			}

			// Rule 2: Wildcards are next to last.
			const aIsWildcard = domainA.includes("*");
			const bIsWildcard = domainB.includes("*");
			if (aIsWildcard !== bIsWildcard) {
				return aIsWildcard ? 1 : -1;
			}

			// Rule 3: Alphabetical sort.
			return domainA.localeCompare(domainB);
		});
	}, [statusResult]);

	const domains = useMemo(
		() => corsStatuses.map((s) => s.domain),
		[corsStatuses]
	);

	const domainListItems = useMemo<DomainListItem[]>(() => {
		return corsStatuses.map((status) => {
			const isProxy = status.preflight_handling === "proxy_decision";
			return {
				domain: status.domain,
				badge: {
					icon: isProxy ? RouteIcon : RouteOff,
					text: isProxy ? "Vane Proxy" : "Origin Server",
				},
			};
		});
	}, [corsStatuses]);

	const {
		data: allConfigs,
		isLoading: areConfigsLoading,
		isError: isConfigsError,
		error: configsError,
	} = useQuery<Record<string, CorsConfig>>({
		queryKey: ["instance", instanceId, "cors", "config", "all"],
		queryFn: async () => {
			const promises = domains.map(async (domain) => {
				const res = await getCorsConfig(instanceId, domain);
				return { domain, config: res.data };
			});
			const results = await Promise.all(promises);
			return results.reduce(
				(acc, { domain, config }) => {
					if (config) acc[domain] = config;
					return acc;
				},
				{} as Record<string, CorsConfig>
			);
		},
		enabled: domains.length > 0,
	});

	const overviewStats = useMemo<CorsOverviewStats>(() => {
		const total = domains.length;
		const proxyHandled = corsStatuses.filter(
			(s) => s.preflight_handling === "proxy_decision"
		).length;
		const wildcardOrigins = allConfigs
			? Object.values(allConfigs).filter((c) => c.allow_origins.includes("*"))
					.length
			: 0;
		return {
			total,
			proxyHandled,
			originHandled: total - proxyHandled,
			wildcardOrigins,
		};
	}, [domains, corsStatuses, allConfigs]);

	const selectedCorsConfigQuery = useQuery<RequestResult<CorsConfig>>({
		queryKey: ["instance", instanceId, "cors", "config", selectedDomain],
		queryFn: () => getCorsConfig(instanceId, selectedDomain!),
		enabled: !!selectedDomain,
	});

	const handleDomainSelect = useCallback(
		(newDomain: string | null) => {
			navigate({
				to: "/$instance/cors-management/$domain",
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
		RequestResult<CorsConfig>,
		Error,
		{ domain: string; config: CorsConfig }
	>({
		mutationFn: (vars) =>
			updateCorsConfig(instanceId, vars.domain, vars.config),
		onSuccess: () =>
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "cors"],
			}),
	});
	const resetMutation = useMutation<RequestResult<unknown>, Error, string>({
		mutationFn: (domain) => resetCorsConfig(instanceId, domain),
		onSuccess: (_, deletedDomain) => {
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "cors"],
			});
			if (selectedDomain === deletedDomain) {
				handleDomainSelect(null);
			}
		},
	});

	const isLoading = isListLoading || (domains.length > 0 && areConfigsLoading);
	const isError = isListError || isConfigsError;
	const error = listError || configsError;

	if (isLoading)
		return <StatusCard icon={Server} text="Loading CORS Configurations..." />;
	if (isError)
		return (
			<StatusCard
				icon={ServerCrash}
				text={error?.message || "Failed to fetch CORS configurations."}
				isError
			/>
		);

	return (
		<Tooltip.Provider delayDuration={200}>
			<div className="space-y-6">
				<CorsOverviewCard stats={overviewStats} />
				<DomainListCard
					title="Domain CORS Policies"
					icon={ArrowRightLeft}
					items={domainListItems}
					selectedDomain={selectedDomain}
					onSelectDomain={handleDomainSelect}
				/>
				{selectedDomain && (
					<CorsEditorCard
						key={selectedDomain}
						domain={selectedDomain}
						query={selectedCorsConfigQuery}
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
