/* src/routes/$instance/custom-header/index.tsx */

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
import { HeaderListCard } from "~/components/custom-header/header-list-card";
import { HeaderEditorCard } from "~/components/custom-header/header-editor-card";
import * as Tooltip from "@radix-ui/react-tooltip";

// --- API Helper Functions ---

// Borrowing from domains page to get the list of domains
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

export const Route = createFileRoute("/$instance/custom-header/")({
	component: HeaderPage,
});

function HeaderPage() {
	const { instance: instanceId } = useParams({
		from: "/$instance/custom-header/",
	});
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

	// --- Step 2: Query for the selected domain's header config ---
	const selectedHeaderConfigQuery = useQuery<RequestResult<HeaderConfig>>({
		queryKey: ["instance", instanceId, "headers", selectedDomain],
		queryFn: () => getHeaderConfig(instanceId, selectedDomain!),
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

	// --- Mutations for header management ---
	const updateMutation = useMutation<
		RequestResult<HeaderConfig>,
		Error,
		{ domain: string; config: HeaderConfig }
	>({
		mutationFn: (vars) =>
			updateHeaderConfig(instanceId, vars.domain, vars.config),
		onSuccess: (_, vars) => {
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "headers", vars.domain],
			});
		},
	});

	const resetMutation = useMutation<RequestResult<unknown>, Error, string>({
		mutationFn: (domain) => resetHeaderConfig(instanceId, domain),
		onSuccess: (_, domain) => {
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "headers", domain],
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
				<HeaderListCard
					domains={domains}
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
