/* src/routes/$instance/domains/$domain.tsx */

import {
	createFileRoute,
	useParams,
	useNavigate,
} from "@tanstack/react-router";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Server, ServerCrash, GitMerge } from "lucide-react";
import React, { useMemo, useCallback } from "react";
import { type RequestResult } from "~/api/request";
import { getInstance, postInstance, deleteInstance } from "~/api/instance";
import { FloatingDomainManager } from "~/components/domain/floating-domain-manager";

// --- API Helper Functions (no changes) ---
async function listDomains(
	instanceId: string
): Promise<RequestResult<ListDomainsResponse>> {
	return getInstance(instanceId, "/v1/domains");
}
async function createDomain(
	instanceId: string,
	domain: string
): Promise<RequestResult<unknown>> {
	const encodedDomain = encodeURIComponent(domain);
	return postInstance(instanceId, `/v1/domains/${encodedDomain}`, {});
}
async function deleteDomain(
	instanceId: string,
	domain: string
): Promise<RequestResult<unknown>> {
	const encodedDomain = encodeURIComponent(domain);
	return deleteInstance(instanceId, `/v1/domains/${encodedDomain}`);
}

// --- Data Types ---
export interface ListDomainsResponse {
	domains: string[];
}

// --- Ensures consistent ordering for domains ---
function sortDomainsList(domains: string[]): string[] {
	return [...domains].sort((a, b) => {
		const isAFallback = a === "fallback";
		const isBFallback = b === "fallback";
		const isAWildcard = a.includes("*");
		const isBWildcard = b.includes("*");

		// Rule 1: "fallback" is always last.
		if (isAFallback !== isBFallback) {
			return isAFallback ? 1 : -1;
		}

		// Rule 2: Wildcards come after regular domains but before "fallback".
		if (isAWildcard !== isBWildcard) {
			return isAWildcard ? 1 : -1;
		}

		// Rule 3: Alphabetical sort for domains of the same type.
		return a.localeCompare(b);
	});
}

export const Route = createFileRoute("/$instance/domains/$domain")({
	component: DomainDetailPage,
});

function DomainDetailPage() {
	const { instance: instanceId, domain } = useParams({
		from: "/$instance/domains/$domain",
	});
	const selectedDomain = domain === "_" ? null : domain;
	const queryClient = useQueryClient();
	const navigate = useNavigate({ from: "/$instance/domains/$domain" });

	const {
		data: domainsResult,
		isLoading,
		isError,
		error,
	} = useQuery<RequestResult<ListDomainsResponse>>({
		queryKey: ["instance", instanceId, "domains"],
		queryFn: () => listDomains(instanceId),
	});

	const domains = useMemo(() => {
		// --- FIX: Apply consistent sorting logic to the domain list ---
		const unsortedDomains = domainsResult?.data?.domains ?? [];
		return sortDomainsList(unsortedDomains);
	}, [domainsResult]);

	const handleDomainSelect = useCallback(
		(newDomain: string) => {
			navigate({
				to: "/$instance/domains/$domain",
				params: { instance: instanceId, domain: newDomain },
				replace: true,
			});
		},
		[navigate, instanceId]
	);

	const addMutation = useMutation<RequestResult<unknown>, Error, string>({
		mutationFn: (newDomain) => createDomain(instanceId, newDomain),
		onSuccess: (_, newDomain) => {
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "domains"],
			});
			handleDomainSelect(newDomain);
		},
	});

	const removeMutation = useMutation<RequestResult<unknown>, Error, string>({
		mutationFn: (domainToDelete) => deleteDomain(instanceId, domainToDelete),
		onSuccess: () => {
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "domains"],
			});
			navigate({ to: "/$instance/domains", params: { instance: instanceId } });
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
		<div className="relative h-full min-h-[calc(100vh-10rem)] w-full">
			{/* Canvas Placeholder */}
			<div className="flex h-full w-full items-center justify-center rounded-lg border-2 border-dashed border-[var(--color-bg-alt)]">
				<div className="flex flex-col items-center gap-4 text-center">
					<GitMerge size={32} className="text-[var(--color-subtext)]" />
					<p className="text-[var(--color-subtext)]">
						Canvas for{" "}
						<span className="font-semibold text-[var(--color-text)]">
							{selectedDomain ?? "selected domain"}
						</span>{" "}
						will be here.
					</p>
				</div>
			</div>

			{/* Floating Domain Manager */}
			<FloatingDomainManager
				domains={domains}
				selectedDomain={selectedDomain}
				onSelectDomain={handleDomainSelect}
				addMutation={addMutation}
				removeMutation={removeMutation}
			/>
		</div>
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
