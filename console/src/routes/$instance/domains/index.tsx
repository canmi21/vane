/* src/routes/$instance/domains/index.tsx */

import { createFileRoute, useParams } from "@tanstack/react-router";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Server, ServerCrash } from "lucide-react";
import React, { useState, useEffect, useMemo } from "react";
import { type RequestResult } from "~/api/request";
import { getInstance, postInstance, deleteInstance } from "~/api/instance";
import { DomainHeaderCard } from "~/components/domain/domain-header-card";

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
	// BUG FIX: The domain must be URL-encoded to handle special characters like '*'.
	const encodedDomain = encodeURIComponent(domain);
	return postInstance(instanceId, `/v1/domains/${encodedDomain}`, {});
}

async function deleteDomain(
	instanceId: string,
	domain: string
): Promise<RequestResult<unknown>> {
	// BUG FIX: The domain must be URL-encoded to handle special characters like '*'.
	const encodedDomain = encodeURIComponent(domain);
	return deleteInstance(instanceId, `/v1/domains/${encodedDomain}`);
}

// --- Data Types ---
export interface ListDomainsResponse {
	domains: string[];
}

export const Route = createFileRoute("/$instance/domains/")({
	component: DomainsPage,
});

function DomainsPage() {
	const { instance: instanceId } = useParams({ from: "/$instance/domains/" });
	const queryClient = useQueryClient();
	const [selectedDomain, setSelectedDomain] = useState<string | null>(null);

	const {
		data: domainsResult,
		isLoading,
		isError,
		error,
	} = useQuery<RequestResult<ListDomainsResponse>>({
		queryKey: ["instance", instanceId, "domains"],
		queryFn: () => listDomains(instanceId),
	});

	const domains = useMemo(
		() => domainsResult?.data?.domains ?? [],
		[domainsResult]
	);

	// Effect to select the first domain when the list loads or changes.
	useEffect(() => {
		if (domains.length > 0 && !domains.includes(selectedDomain ?? "")) {
			setSelectedDomain(domains[0]);
		} else if (domains.length === 0) {
			setSelectedDomain(null);
		}
	}, [domains, selectedDomain]);

	const addMutation = useMutation<RequestResult<unknown>, Error, string>({
		mutationFn: (newDomain) => createDomain(instanceId, newDomain),
		onSuccess: (_, newDomain) => {
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "domains"],
			});
			// Select the newly created domain after the list refetches.
			setSelectedDomain(newDomain);
		},
	});

	const removeMutation = useMutation<RequestResult<unknown>, Error, string>({
		mutationFn: (domain) => deleteDomain(instanceId, domain),
		onSuccess: () => {
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "domains"],
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
		<div className="w-full">
			<DomainHeaderCard
				domains={domains}
				selectedDomain={selectedDomain}
				setSelectedDomain={setSelectedDomain}
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
