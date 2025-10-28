/* src/routes/$instance/domains/$domain.tsx */

import { createFileRoute, useParams } from "@tanstack/react-router";
import { Server, ServerCrash, Loader2 } from "lucide-react";
import React from "react";
import { DomainCanvas } from "~/components/domain/domain-canvas";
import { FloatingDomainManager } from "~/components/domain/floating-domain-manager";
import { useDomainData } from "~/hooks/use-domain-data";
import { useCanvasLayout } from "~/hooks/use-canvas-layout";

export const Route = createFileRoute("/$instance/domains/$domain")({
	component: DomainDetailPage,
});

function DomainDetailPage() {
	const { instance: instanceId, domain } = useParams({
		from: "/$instance/domains/$domain",
	});
	const selectedDomain = domain === "_" ? null : domain;

	const {
		domains,
		domainsQuery,
		addMutation,
		removeMutation,
		handleDomainSelect,
	} = useDomainData(instanceId, selectedDomain);

	const { layout, handleLayoutChange, addNode } = useCanvasLayout({
		selectedDomain,
	});

	if (domainsQuery.isLoading) {
		return <FullPageStatus icon={Server} text="Loading Domains..." />;
	}
	if (domainsQuery.isError) {
		return (
			<FullPageStatus
				icon={ServerCrash}
				text={domainsQuery.error.message}
				isError
			/>
		);
	}

	return (
		<div className="h-full w-full">
			{layout && selectedDomain ? (
				<DomainCanvas
					layout={layout}
					onLayoutChange={handleLayoutChange}
					selectedDomain={selectedDomain}
					onAddNode={addNode}
				/>
			) : (
				<FullPageStatus icon={Loader2} text="Loading Canvas Layout..." />
			)}
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

function FullPageStatus({
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
		<div className="flex h-full w-full items-center justify-center">
			<div className="flex w-fit flex-col items-center gap-4 p-12">
				<Icon
					size={32}
					className={`${colorClass} ${!isError ? "animate-spin" : ""}`}
				/>
				<p className={`text-center font-medium ${colorClass}`}>{text}</p>
			</div>
		</div>
	);
}
