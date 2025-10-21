/* src/routes/$instance/custom-header/$domain.tsx */

import {
	createFileRoute,
	useParams,
	useNavigate,
} from "@tanstack/react-router";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Server, ServerCrash, ListPlus } from "lucide-react";
import React, { useMemo, useCallback } from "react";
import { type RequestResult } from "~/api/request";
import { getInstance, putInstance, deleteInstance } from "~/api/instance";
import {
	DomainListCard,
	type DomainListItem,
} from "~/components/shared/domain-list-card";
import { HeaderEditorCard } from "~/components/custom-header/header-editor-card";
import * as Tooltip from "@radix-ui/react-tooltip";

// --- API Helper Functions ---
async function listDomains(
	instanceId: string
): Promise<RequestResult<{ domains: string[] }>> {
	return getInstance(instanceId, "/v1/domains");
}
async function getHeaderConfig(
	instanceId: string,
	domain: string
): Promise<RequestResult<HeaderConfig>> {
	return getInstance(instanceId, `/v1/headers/${domain}`);
}
async function updateHeaderConfig(
	instanceId: string,
	domain: string,
	config: HeaderConfig
): Promise<RequestResult<HeaderConfig>> {
	return putInstance(instanceId, `/v1/headers/${domain}`, config as never);
}
async function resetHeaderConfig(
	instanceId: string,
	domain: string
): Promise<RequestResult<unknown>> {
	return deleteInstance(instanceId, `/v1/headers/${domain}`);
}

// --- Data Types from Backend ---
export interface HeaderConfig {
	headers: Record<string, string>;
}

export const Route = createFileRoute("/$instance/custom-header/$domain")({
	component: HeaderDetailPage,
});

function HeaderDetailPage() {
	const { instance: instanceId, domain } = useParams({
		from: "/$instance/custom-header/$domain",
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
		Record<string, HeaderConfig>
	>({
		queryKey: ["instance", instanceId, "headers", "all"],
		queryFn: async () => {
			const promises = domains.map(async (domain) => {
				const res = await getHeaderConfig(instanceId, domain);
				return { domain, config: res.data };
			});
			const results = await Promise.all(promises);
			return results.reduce(
				(acc, { domain, config }) => {
					if (config) acc[domain] = config;
					return acc;
				},
				{} as Record<string, HeaderConfig>
			);
		},
		enabled: domains.length > 0,
	});

	const domainListItems = useMemo<DomainListItem[]>(() => {
		return domains.map((domain) => {
			const config = allConfigs?.[domain];
			const count = config ? Object.keys(config.headers).length : 0;
			return {
				domain,
				badge: {
					icon: ListPlus,
					text: `${count} Header(s)`,
				},
			};
		});
	}, [domains, allConfigs]);

	const selectedHeaderConfigQuery = useQuery<RequestResult<HeaderConfig>>({
		queryKey: ["instance", instanceId, "headers", selectedDomain],
		queryFn: () => getHeaderConfig(instanceId, selectedDomain!),
		enabled: !!selectedDomain,
	});

	const handleDomainSelect = useCallback(
		(newDomain: string | null) => {
			navigate({
				to: "/$instance/custom-header/$domain",
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
		RequestResult<HeaderConfig>,
		Error,
		{ domain: string; config: HeaderConfig }
	>({
		mutationFn: (vars) =>
			updateHeaderConfig(instanceId, vars.domain, vars.config),
		onSuccess: () =>
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "headers"],
			}),
	});
	const resetMutation = useMutation<RequestResult<unknown>, Error, string>({
		mutationFn: (domain) => resetHeaderConfig(instanceId, domain),
		onSuccess: (_, deletedDomain) => {
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "headers"],
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
					title="Custom Response Headers"
					icon={ListPlus}
					items={domainListItems}
					selectedDomain={selectedDomain}
					onSelectDomain={handleDomainSelect}
				/>
				{selectedDomain && (
					<HeaderEditorCard
						key={selectedDomain}
						domain={selectedDomain}
						query={selectedHeaderConfigQuery}
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
