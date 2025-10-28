/* src/hooks/use-domain-data.ts */

import { useMemo } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { getInstance, postInstance, deleteInstance } from "~/api/instance";
import { type RequestResult } from "~/api/request";

// --- Type Definitions ---
interface ListDomainsResponse {
	domains: string[];
}
interface RateLimitConfig {
	requests_per_second: number;
}

// --- API Functions ---
const listDomains = (instanceId: string) =>
	getInstance<ListDomainsResponse>(instanceId, "/v1/domains");

const getRateLimitConfig = (instanceId: string, domain: string) =>
	getInstance<RateLimitConfig>(instanceId, `/v1/ratelimit/${domain}`);

const createDomain = (instanceId: string, domain: string) =>
	postInstance(instanceId, `/v1/domains/${encodeURIComponent(domain)}`, {});

const deleteDomain = (instanceId: string, domain: string) =>
	deleteInstance(instanceId, `/v1/domains/${encodeURIComponent(domain)}`);

// --- Helper Functions ---
const sortDomainsList = (domains: string[]): string[] => {
	// Sorts domains, keeping "fallback" and wildcards at the end.
	return [...domains].sort((a, b) => {
		const isAFallback = a === "fallback";
		const isBFallback = b === "fallback";
		if (isAFallback !== isBFallback) return isAFallback ? 1 : -1;
		const isAWildcard = a.includes("*");
		const isBWildcard = b.includes("*");
		if (isAWildcard !== isBWildcard) return isAWildcard ? 1 : -1;
		return a.localeCompare(b);
	});
};

/**
 * A hook to manage all domain-related data fetching and mutations for the canvas page.
 */
export function useDomainData(
	instanceId: string,
	selectedDomain: string | null
) {
	const queryClient = useQueryClient();
	const navigate = useNavigate();

	const domainsQuery = useQuery({
		queryKey: ["instance", instanceId, "domains"],
		queryFn: () => listDomains(instanceId),
	});

	const rateLimitQuery = useQuery({
		queryKey: ["instance", instanceId, "ratelimit", selectedDomain],
		queryFn: () => getRateLimitConfig(instanceId, selectedDomain!),
		enabled: !!selectedDomain && !domainsQuery.isLoading,
	});

	const domains = useMemo(() => {
		const apiDomains = domainsQuery.data?.data?.domains ?? [];
		return sortDomainsList(apiDomains);
	}, [domainsQuery.data]);

	const handleDomainSelect = (newDomain: string) => {
		navigate({
			to: "/$instance/domains/$domain",
			params: { instance: instanceId, domain: newDomain },
			replace: true,
		});
	};

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

	return {
		domains,
		domainsQuery,
		rateLimitQuery,
		addMutation,
		removeMutation,
		handleDomainSelect,
	};
}
