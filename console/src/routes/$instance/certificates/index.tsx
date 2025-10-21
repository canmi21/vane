/* src/routes/$instance/certificates/index.tsx */

import { createFileRoute, useParams, Navigate } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { Server, ServerCrash } from "lucide-react";
import { getInstance } from "~/api/instance";
import type { ListCertsResponse } from "./$domain";

// This function is only used here to find the first certificate for redirection.
async function listCerts(instanceId: string) {
	return getInstance<ListCertsResponse>(instanceId, "/v1/certs");
}

export const Route = createFileRoute("/$instance/certificates/")({
	component: CertificatesIndexPage,
});

function CertificatesIndexPage() {
	const { instance: instanceId } = useParams({
		from: "/$instance/certificates/",
	});

	const { data, isLoading, isError, error, isSuccess } = useQuery({
		queryKey: ["instance", instanceId, "certs", "list"],
		queryFn: () => listCerts(instanceId),
		// We don't want this query to be stale for too long, as it's just for a redirect.
		staleTime: 1000 * 10,
	});

	if (isLoading) {
		return <StatusCard icon={Server} text="Loading certificates..." />;
	}

	if (isError) {
		return <StatusCard icon={ServerCrash} text={error.message} isError />;
	}

	if (isSuccess) {
		const unsortedDomains = Object.keys(data?.data?.certificates ?? {});

		// --- FIX: Apply the same sorting logic as the main page before redirecting ---
		const certDomains = unsortedDomains.sort((a, b) => {
			const aIsWildcard = a.includes("*");
			const bIsWildcard = b.includes("*");

			// If one is a wildcard and the other is not, the wildcard goes last.
			if (aIsWildcard !== bIsWildcard) {
				return aIsWildcard ? 1 : -1;
			}

			// Otherwise, sort alphabetically.
			return a.localeCompare(b);
		});

		// Now, redirect to the first item of the *sorted* list.
		const targetDomain = certDomains.length > 0 ? certDomains[0] : "_";

		return (
			<Navigate
				to="/$instance/certificates/$domain"
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
