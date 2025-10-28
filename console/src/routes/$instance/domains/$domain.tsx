/* src/routes/$instance/domains/$domain.tsx */

import {
	createFileRoute,
	useNavigate,
	useParams,
} from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Server, ServerCrash, Loader2 } from "lucide-react";
import React, { useCallback, useMemo, useState, useEffect } from "react";
import { deleteInstance, getInstance, postInstance } from "~/api/instance";
import { type RequestResult } from "~/api/request";
import { DomainCanvas } from "~/components/domain/domain-canvas";
import { FloatingDomainManager } from "~/components/domain/floating-domain-manager";
import {
	loadLayout,
	saveLayout,
	type CanvasLayout,
	type CanvasNode,
} from "~/lib/canvas-layout";

// --- API & Data Types ---
interface ListDomainsResponse {
	domains: string[];
}
async function listDomains(
	instanceId: string
): Promise<RequestResult<ListDomainsResponse>> {
	return getInstance(instanceId, "/v1/domains");
}
async function getRateLimitConfig(
	instanceId: string,
	domain: string
): Promise<RequestResult<{ requests_per_second: number }>> {
	return getInstance(instanceId, `/v1/ratelimit/${domain}`);
}
const createDomain = (instanceId: string, domain: string) =>
	postInstance(instanceId, `/v1/domains/${encodeURIComponent(domain)}`, {});
const deleteDomain = (instanceId: string, domain: string) =>
	deleteInstance(instanceId, `/v1/domains/${encodeURIComponent(domain)}`);

function sortDomainsList(domains: string[]): string[] {
	return [...domains].sort((a, b) => {
		const isAFallback = a === "fallback";
		const isBFallback = b === "fallback";
		if (isAFallback !== isBFallback) return isAFallback ? 1 : -1;
		const isAWildcard = a.includes("*");
		const isBWildcard = b.includes("*");
		if (isAWildcard !== isBWildcard) return isAWildcard ? 1 : -1;
		return a.localeCompare(b);
	});
}

export const Route = createFileRoute("/$instance/domains/$domain")({
	component: DomainDetailPage,
});

function DomainDetailPage() {
	const { instance: instanceId, domain } = useParams({
		from: "/$instance/domains/$domain",
	});
	const selectedDomain = domain === "_" ? null : domain;
	const queryClient = useQueryClient();
	const navigate = useNavigate();

	const [layout, setLayout] = useState<CanvasLayout | null>(null);

	const domainsQuery = useQuery({
		queryKey: ["instance", instanceId, "domains"],
		queryFn: () => listDomains(instanceId),
	});

	const rateLimitQuery = useQuery({
		queryKey: ["instance", instanceId, "ratelimit", selectedDomain],
		queryFn: () => getRateLimitConfig(instanceId, selectedDomain!),
		enabled: !!selectedDomain && !domainsQuery.isLoading,
	});

	// --- FIX: Wrap generateDefaultLayout in useCallback ---
	const generateDefaultLayout = useCallback(() => {
		let nextX = 150;
		const nodes: CanvasNode[] = [
			{ id: "entry-point", type: "entry-point", x: nextX, y: 200 },
		];
		const connections = [];
		nextX += 350;

		const isRateLimitEnabled =
			(rateLimitQuery.data?.data?.requests_per_second ?? 0) > 0;
		if (isRateLimitEnabled) {
			nodes.push({ id: "rate-limit", type: "rate-limit", x: nextX, y: 200 });
			connections.push({
				id: "entry-to-ratelimit",
				fromNodeId: "entry-point",
				fromHandle: "output",
				toNodeId: "rate-limit",
				toHandle: "input",
			});
		}
		return { nodes, connections };
	}, [rateLimitQuery.data]);

	useEffect(() => {
		if (!selectedDomain || rateLimitQuery.isLoading) return;

		const savedLayout = loadLayout(selectedDomain);
		if (savedLayout) {
			const shouldHaveRateLimit =
				(rateLimitQuery.data?.data?.requests_per_second ?? 0) > 0;
			const savedHasRateLimit = savedLayout.nodes.some(
				(n) => n.type === "rate-limit"
			);

			if (shouldHaveRateLimit !== savedHasRateLimit) {
				localStorage.removeItem(`@vane/canvas-layout/${selectedDomain}`);
				const newLayout = generateDefaultLayout();
				setLayout(newLayout);
				saveLayout(selectedDomain, newLayout);
			} else {
				setLayout(savedLayout);
			}
			return;
		}

		const newLayout = generateDefaultLayout();
		setLayout(newLayout);
		saveLayout(selectedDomain, newLayout);

		// --- FIX: Add generateDefaultLayout to the dependency array ---
	}, [
		selectedDomain,
		rateLimitQuery.isLoading,
		rateLimitQuery.data,
		generateDefaultLayout,
	]);

	const handleLayoutChange = useCallback(
		(newLayout: CanvasLayout) => {
			setLayout(newLayout);
			if (selectedDomain) saveLayout(selectedDomain, newLayout);
		},
		[selectedDomain]
	);

	const domains = useMemo(() => {
		const apiDomains = domainsQuery.data?.data?.domains ?? [];
		return sortDomainsList(apiDomains);
	}, [domainsQuery.data]);

	const handleDomainSelect = useCallback(
		(newDomain: string) => {
			setLayout(null);
			navigate({
				to: "/$instance/domains/$domain",
				params: { instance: instanceId, domain: newDomain },
				replace: true,
			});
		},
		[navigate, instanceId]
	);

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

	if (domainsQuery.isLoading)
		return (
			<FullPageStatus icon={Server} text="Loading Domains..." isError={false} />
		);
	if (domainsQuery.isError)
		return (
			<FullPageStatus
				icon={ServerCrash}
				text={domainsQuery.error.message}
				isError
			/>
		);

	return (
		<div className="h-full w-full">
			{layout && selectedDomain ? (
				<DomainCanvas
					layout={layout}
					onLayoutChange={handleLayoutChange}
					selectedDomain={selectedDomain}
				/>
			) : (
				<FullPageStatus
					icon={Loader2}
					text="Loading Canvas Layout..."
					isError={false}
				/>
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
	isError,
}: {
	icon: React.ElementType;
	text: string;
	isError: boolean;
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
