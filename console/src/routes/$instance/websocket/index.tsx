/* src/routes/$instance/websocket/index.tsx */

import { createFileRoute, useParams, Navigate } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { Server, ServerCrash } from "lucide-react";
import { getInstance } from "~/api/instance";

// Minimal API call for redirection
async function listDomains(instanceId: string): Promise<{ domains: string[] }> {
	const result = await getInstance<{ domains: string[] }>(
		instanceId,
		"/v1/domains"
	);
	return result.data ?? { domains: [] };
}

export const Route = createFileRoute("/$instance/websocket/")({
	component: WebSocketIndexPage,
});

function WebSocketIndexPage() {
	const { instance: instanceId } = useParams({
		from: "/$instance/websocket/",
	});

	const { data, isLoading, isError, error, isSuccess, isFetching } = useQuery({
		queryKey: ["instance", instanceId, "domains"],
		queryFn: () => listDomains(instanceId),
		staleTime: 0, // Always refetch to avoid stale cache issues on navigation
	});

	if (isLoading || isFetching) {
		return <StatusCard icon={Server} text="Loading domains..." />;
	}

	if (isError) {
		return <StatusCard icon={ServerCrash} text={error.message} isError />;
	}

	if (isSuccess) {
		const apiDomains = data.domains ?? [];

		const sortedApiDomains = apiDomains.sort((a, b) => {
			const aIsWildcard = a.includes("*");
			const bIsWildcard = b.includes("*");
			if (aIsWildcard !== bIsWildcard) {
				return aIsWildcard ? 1 : -1;
			}
			return a.localeCompare(b);
		});

		const targetDomain =
			sortedApiDomains.length > 0 ? sortedApiDomains[0] : "fallback";

		return (
			<Navigate
				to="/$instance/websocket/$domain"
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
