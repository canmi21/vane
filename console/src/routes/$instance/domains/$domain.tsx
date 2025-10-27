/* src/routes/$instance/domains/$domain.tsx */

import {
	createFileRoute,
	useNavigate,
	useParams,
} from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Server, ServerCrash } from "lucide-react";
import React, { useCallback, useMemo } from "react";
import { deleteInstance, getInstance, postInstance } from "~/api/instance";
import { type RequestResult } from "~/api/request";
import { DomainCanvas } from "~/components/domain/domain-canvas";
import { FloatingDomainManager } from "~/components/domain/floating-domain-manager";

// --- API Helper Functions ---
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

		if (isAFallback !== isBFallback) return isAFallback ? 1 : -1;
		if (isAWildcard !== isBWildcard) return isAWildcard ? 1 : -1;
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
		return (
			<div className="flex h-full w-full items-center justify-center">
				<StatusCard icon={Server} text="Loading Domains..." />
			</div>
		);
	}
	if (isError) {
		return (
			<div className="flex h-full w-full items-center justify-center">
				<StatusCard
					icon={ServerCrash}
					text={error?.message || "Failed to fetch domains."}
					isError
				/>
			</div>
		);
	}

	return (
		<div className="h-full w-full">
			<DomainCanvas>
				{/* Canvas is now empty and ready for future content */}
			</DomainCanvas>
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
		<div className="flex w-fit items-center justify-center rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] p-12 shadow-sm">
			<div className="flex flex-col items-center gap-4">
				<Icon size={32} className={colorClass} />
				<p className={`text-center font-medium ${colorClass}`}>{text}</p>
			</div>
		</div>
	);
}
