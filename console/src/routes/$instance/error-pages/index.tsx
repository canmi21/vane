/* src/routes/$instance/error-pages/index.tsx */

import { createFileRoute, useParams, Navigate } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { Server, ServerCrash } from "lucide-react";
import { getInstance } from "~/api/instance";
import type { ListTemplatesResponse } from "./$page";

// This function is only used here to find the first template for redirection.
async function listTemplates(instanceId: string) {
	return getInstance<ListTemplatesResponse>(instanceId, "/v1/templates");
}

export const Route = createFileRoute("/$instance/error-pages/")({
	component: ErrorPagesIndexPage,
});

function ErrorPagesIndexPage() {
	const { instance: instanceId } = useParams({
		from: "/$instance/error-pages/",
	});

	const { data, isLoading, isError, error, isSuccess } = useQuery({
		queryKey: ["instance", instanceId, "templates"],
		queryFn: () => listTemplates(instanceId),
		staleTime: 1000 * 10,
	});

	if (isLoading) {
		return <StatusCard icon={Server} text="Loading templates..." />;
	}

	if (isError) {
		return <StatusCard icon={ServerCrash} text={error.message} isError />;
	}

	if (isSuccess) {
		// Clean and sort the template names alphabetically
		const templates =
			data?.data?.templates
				.map((t) => t.replace(".html", ""))
				.sort((a, b) => a.localeCompare(b)) ?? [];

		// If templates exist, navigate to the first one.
		// If not, navigate to a special "_" route that the main component will interpret as "no selection".
		const targetPage = templates.length > 0 ? templates[0] : "_";

		return (
			<Navigate
				to="/$instance/error-pages/$page"
				params={{
					instance: instanceId,
					page: targetPage,
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
