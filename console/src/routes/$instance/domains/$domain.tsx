/* src/routes/$instance/domains/$domain.tsx */

import { createFileRoute, useParams } from "@tanstack/react-router";
import { Server, ServerCrash, Loader2, Puzzle } from "lucide-react";
import React from "react"; // --- FINAL FIX: Import React for JSX ---
import { DomainCanvas } from "~/components/domain/domain-canvas";
import { FloatingDomainManager } from "~/components/domain/floating-domain-manager";
import { useDomainData } from "~/hooks/use-domain-data";
import { useCanvasLayout } from "~/hooks/use-canvas-layout";
import { usePluginData } from "~/hooks/use-plugin-data";

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
	const pluginsQuery = usePluginData(instanceId);

	const { layout, handleLayoutChange, addNode, updateNodeData } =
		useCanvasLayout({
			selectedDomain,
		});

	if (domainsQuery.isLoading || pluginsQuery.isLoading) {
		return <FullPageStatus icon={Server} text="Loading Configuration..." />;
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
	if (pluginsQuery.isError) {
		return (
			<FullPageStatus icon={Puzzle} text={pluginsQuery.error.message} isError />
		);
	}

	const allPlugins = [
		...(pluginsQuery.data?.data?.internal ?? []),
		...(pluginsQuery.data?.data?.external ?? []),
	];

	return (
		<div className="h-full w-full">
			{layout && selectedDomain ? (
				<DomainCanvas
					layout={layout}
					onLayoutChange={handleLayoutChange}
					selectedDomain={selectedDomain}
					plugins={allPlugins}
					onAddNode={addNode}
					onUpdateNodeData={updateNodeData}
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

// --- FINAL FIX: Restore the FullPageStatus component definition ---
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
