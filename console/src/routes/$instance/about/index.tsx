/* src/routes/$instance/about/index.tsx */

import { createFileRoute, useParams } from "@tanstack/react-router";
import { getInstance, getActiveInstanceId } from "~/api/instance";
import { useQuery } from "@tanstack/react-query";
import { Server, AlertTriangle } from "lucide-react";
import { type RequestResult } from "~/api/request";
import React from "react";
import { HeroCard } from "~/components/about/hero-card";
import { RuntimeCard } from "~/components/about/runtime-card";
import { OpenSourceCard } from "~/components/about/open-source-card";

// --- Export Data Types for consumption by child components ---
export interface RootInfo {
	package: {
		version: string;
		author: string;
		license: string;
		repository: string;
	};
	build: { cargo: string; rust: string; vane: string };
	runtime: { platform: string; arch: string };
	timestamp: string;
}
export interface InstanceInfo {
	instance_id: string;
	created_at: string;
}

export const Route = createFileRoute("/$instance/about/")({
	component: AboutPage,
});

function AboutPage() {
	const { instance: instanceId } = useParams({ from: "/$instance/about/" });

	const { data: rootResult, isLoading: isRootLoading } = useQuery<
		RequestResult<RootInfo>
	>({
		queryKey: ["instance", instanceId, "root"],
		queryFn: () => getInstance(instanceId, "/"),
	});
	const { data: instanceResult, isLoading: isInstanceLoading } = useQuery<
		RequestResult<InstanceInfo>
	>({
		queryKey: ["instance", instanceId, "info"],
		queryFn: () => getInstance(instanceId, "/v1/instance"),
	});

	const isLoading = isRootLoading || isInstanceLoading;
	const error = rootResult?.message || instanceResult?.message;
	const isActive = instanceId === getActiveInstanceId();

	if (isLoading)
		return <StatusCard icon={Server} text="Loading Instance Info..." />;
	if (error) return <StatusCard icon={AlertTriangle} text={error} isError />;

	return (
		<div className="w-full space-y-6">
			<HeroCard
				rootData={rootResult?.data}
				instanceData={instanceResult?.data}
				isActive={isActive}
			/>
			<div className="grid grid-cols-1 gap-6 lg:grid-cols-2">
				<RuntimeCard rootData={rootResult?.data} />
				<OpenSourceCard rootData={rootResult?.data} />
			</div>
		</div>
	);
}

// --- StatusCard Component (can be moved to a shared folder later) ---
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
