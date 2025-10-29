/* src/routes/$instance/domains/index.tsx */

import { createFileRoute, useParams, Navigate } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { Server, ServerCrash } from "lucide-react";
import { getInstance } from "~/api/instance";
// --- FINAL FIX: Import the type from its actual source file. ---
import { type ListDomainsResponse } from "~/hooks/use-domain-data";

// This function is only used here to find the first domain for redirection.
async function listDomains(instanceId: string) {
	return getInstance<ListDomainsResponse>(instanceId, "/v1/domains");
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

export const Route = createFileRoute("/$instance/domains/")({
	component: DomainsIndexPage,
});

function DomainsIndexPage() {
	const { instance: instanceId } = useParams({ from: "/$instance/domains/" });

	const { data, isLoading, isError, error, isSuccess } = useQuery({
		queryKey: ["instance", instanceId, "domains"],
		queryFn: () => listDomains(instanceId),
		staleTime: 1000 * 10,
	});

	if (isLoading) {
		return <StatusCard icon={Server} text="Loading domains..." />;
	}

	if (isError) {
		return <StatusCard icon={ServerCrash} text={error.message} isError />;
	}

	if (isSuccess) {
		// --- Apply consistent sorting logic before redirection ---
		const unsortedDomains = data?.data?.domains ?? [];
		const domains = sortDomainsList(unsortedDomains);

		// Redirect to the first domain in the *sorted* list.
		const targetDomain = domains.length > 0 ? domains[0] : "_";

		return (
			<Navigate
				to="/$instance/domains/$domain"
				params={{
					instance: instanceId,
					domain: targetDomain,
				}}
				replace
			/>
		);
	}

	return null; // Should not be reached
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
