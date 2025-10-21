/* src/routes/$instance/cors-management/index.tsx */

import { createFileRoute, useParams, Navigate } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { Server, ServerCrash } from "lucide-react";
import { getInstance } from "~/api/instance";
import type { CorsStatus } from "./$domain";

// Minimal API call for redirection
async function listCorsStatus(instanceId: string): Promise<CorsStatus[]> {
	const result = await getInstance<CorsStatus[]>(instanceId, "/v1/cors");
	return result.data ?? [];
}

export const Route = createFileRoute("/$instance/cors-management/")({
	component: CorsManagementIndexPage,
});

function CorsManagementIndexPage() {
	const { instance: instanceId } = useParams({
		from: "/$instance/cors-management/",
	});

	const { data, isLoading, isError, error, isSuccess, isFetching } = useQuery({
		queryKey: ["instance", instanceId, "cors", "status"],
		queryFn: () => listCorsStatus(instanceId),
		staleTime: 0,
	});

	if (isLoading || isFetching) {
		return <StatusCard icon={Server} text="Loading CORS policies..." />;
	}

	if (isError) {
		return <StatusCard icon={ServerCrash} text={error.message} isError />;
	}

	if (isSuccess) {
		const statuses = data ?? [];

		// --- FIX: Apply the universal robust sorting logic ---
		const sortedStatuses = statuses.sort((a, b) => {
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

		const targetDomain =
			sortedStatuses.length > 0 ? sortedStatuses[0].domain : "_";

		return (
			<Navigate
				to="/$instance/cors-management/$domain"
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

// A self-contained StatusCard for this simple page.
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
