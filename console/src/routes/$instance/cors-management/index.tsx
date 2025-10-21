/* src/routes/$instance/cors-management/index.tsx */

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
import {
	CorsOverviewCard,
	type CorsOverviewStats,
} from "~/components/cors/cors-overview-card";
import { CorsListCard } from "~/components/cors/cors-list-card";
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

export const Route = createFileRoute("/$instance/cors-management/")({
	component: CorsPage,
});

function CorsPage() {
	const { instance: instanceId } = useParams({
		from: "/$instance/cors-management/",
	});
	const queryClient = useQueryClient();
	const navigate = useNavigate();
	const location = useLocation();

	const [selectedDomain, setSelectedDomain] = useState<string | null>(null);

	// --- Step 1: Query for the list of domains ---
	const {
		data: statusResult,
		isLoading: isListLoading,
		isError: isListError,
		error: listError,
	} = useQuery<RequestResult<CorsStatus[]>>({
		queryKey: ["instance", instanceId, "cors", "status"],
		queryFn: () => listCorsStatus(instanceId),
	});

	const corsStatuses = useMemo(() => statusResult?.data ?? [], [statusResult]);
	const domains = useMemo(
		() => corsStatuses.map((s) => s.domain),
		[corsStatuses]
	);

	// --- Step 2: Fetch all configs in parallel for overview stats ---
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

	// --- Calculate overview stats ---
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

	// --- This query is for the SELECTED domain's detailed config ---
	const selectedCorsConfigQuery = useQuery<RequestResult<CorsConfig>>({
		queryKey: ["instance", instanceId, "cors", "config", selectedDomain],
		queryFn: () => getCorsConfig(instanceId, selectedDomain!),
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
		if (isListLoading) return;
		const hashDomain = location.hash
			? decodeURIComponent(location.hash.slice(1))
			: null;
		if (hashDomain && domains.includes(hashDomain)) {
			if (selectedDomain !== hashDomain) setSelectedDomain(hashDomain);
			return;
		}
		if (!selectedDomain || !domains.includes(selectedDomain)) {
			if (domains.length > 0) {
				handleDomainSelect(domains[0]);
			} else {
				handleDomainSelect(null);
			}
		}
	}, [
		domains,
		isListLoading,
		location.hash,
		selectedDomain,
		handleDomainSelect,
	]);

	// --- Mutations for CORS management ---
	const updateMutation = useMutation<
		RequestResult<CorsConfig>,
		Error,
		{ domain: string; config: CorsConfig }
	>({
		mutationFn: (vars) =>
			updateCorsConfig(instanceId, vars.domain, vars.config),
		onSuccess: () => {
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "cors"],
			});
		},
	});

	const resetMutation = useMutation<RequestResult<unknown>, Error, string>({
		mutationFn: (domain) => resetCorsConfig(instanceId, domain),
		onSuccess: () => {
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "cors"],
			});
		},
	});

	const isLoading = isListLoading || (domains.length > 0 && areConfigsLoading);
	const isError = isListError || isConfigsError;
	const error = listError || configsError;

	if (isLoading) {
		return <StatusCard icon={Server} text="Loading CORS Configurations..." />;
	}
	if (isError) {
		return (
			<StatusCard
				icon={ServerCrash}
				text={error?.message || "Failed to fetch CORS configurations."}
				isError
			/>
		);
	}

	return (
		<Tooltip.Provider delayDuration={200}>
			<div className="space-y-6">
				<CorsOverviewCard stats={overviewStats} />
				<CorsListCard
					statuses={corsStatuses}
					selectedDomain={selectedDomain}
					onSelectDomain={handleDomainSelect}
				/>
				{/* --- FIX: Pass the correct query prop instead of initialConfig --- */}
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
