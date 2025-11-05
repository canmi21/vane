/* src/routes/$instance/domains/$domain.tsx */

import { createFileRoute, useParams } from "@tanstack/react-router";
import { Server, ServerCrash, Loader2, Puzzle } from "lucide-react";
import React, { useEffect, useRef } from "react";
import { DomainCanvas } from "~/components/domain/domain-canvas";
import { FloatingDomainManager } from "~/components/domain/floating-domain-manager";
import { SyncStatusIndicator } from "~/components/domain/sync-status-indicator";
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

	const domainData = useDomainData(instanceId, selectedDomain);
	const pluginsQuery = usePluginData(instanceId);

	// The hook has been simplified and no longer provides node-specific add functions.
	const { layout, handleLayoutChange, addNode, updateNodeData, syncStatus } =
		useCanvasLayout({
			instanceId,
			selectedDomain,
		});

	// Navigation Guard to prevent losing unsaved changes
	const syncStatusRef = useRef(syncStatus);
	useEffect(() => {
		syncStatusRef.current = syncStatus;
	}, [syncStatus]);

	useEffect(() => {
		const handleBeforeUnload = (e: BeforeUnloadEvent) => {
			if (
				syncStatusRef.current === "unsaved" ||
				syncStatusRef.current === "saving"
			) {
				e.preventDefault();
				e.returnValue =
					"Waiting for layout to update. Are you sure you want to leave?";
			}
		};
		window.addEventListener("beforeunload", handleBeforeUnload);
		return () => window.removeEventListener("beforeunload", handleBeforeUnload);
	}, []);

	// --- Loading and Error states from various hooks ---
	if (domainData.domainsQuery.isLoading || pluginsQuery.isLoading) {
		return <FullPageStatus icon={Server} text="Loading Configuration..." />;
	}
	if (domainData.domainsQuery.isError) {
		return (
			<FullPageStatus
				icon={ServerCrash}
				text={domainData.domainsQuery.error.message}
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
			<SyncStatusIndicator status={syncStatus} />

			{syncStatus === "loading" || !layout ? (
				<FullPageStatus icon={Loader2} text="Loading Canvas Layout..." />
			) : (
				<DomainCanvas
					layout={layout}
					onLayoutChange={handleLayoutChange}
					selectedDomain={selectedDomain!}
					plugins={allPlugins}
					onAddNode={addNode}
					onUpdateNodeData={updateNodeData}
				/>
			)}
			<FloatingDomainManager
				domains={domainData.domains}
				selectedDomain={selectedDomain}
				onSelectDomain={domainData.handleDomainSelect}
				addMutation={domainData.addMutation}
				removeMutation={domainData.removeMutation}
			/>
		</div>
	);
}

// The FullPageStatus component for displaying loading/error states.
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
					className={`${colorClass} ${!isError && text.includes("Loading") ? "animate-spin" : ""}`}
				/>
				<p className={`text-center font-medium ${colorClass}`}>{text}</p>
			</div>
		</div>
	);
}
