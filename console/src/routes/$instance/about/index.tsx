/* src/routes/$instance/about/index.tsx */

import { createFileRoute, useParams } from "@tanstack/react-router";
import { getInstance, getActiveInstanceId } from "~/api/instance";
import { useQuery } from "@tanstack/react-query";
import {
	GitCommitHorizontal,
	Info,
	Cpu,
	Terminal,
	Code,
	Calendar,
	AlertTriangle,
	Package,
	BookMarked,
	Scale,
	Github,
	Clock,
	Layers,
	Server,
	Activity,
	MessagesSquare,
} from "lucide-react";
import { type RequestResult } from "~/api/request";
import React from "react";
import VaneLogo from "~/assets/about.svg";

// --- Data Types and Helpers (Unchanged) ---
interface RootInfo {
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
interface InstanceInfo {
	instance_id: string;
	created_at: string;
}
const parseVersion = (vaneBuild: string = "") =>
	vaneBuild.split(" ")[1] ?? "N/A";
const parseBuildHash = (vaneBuild: string = "") =>
	vaneBuild.split(" ")[2]?.slice(1, -1) ?? "N/A";
const parseToolVersion = (toolString: string = "") =>
	toolString.split(" ")[1] ?? "N/A";
const formatDate = (dateString: string) => {
	try {
		return new Date(dateString).toLocaleDateString("en-US", {
			year: "numeric",
			month: "short",
			day: "numeric",
		});
	} catch {
		return "N/A";
	}
};
const formatDateTime = (dateString: string) => {
	try {
		return new Date(dateString).toLocaleString("en-US", {
			year: "numeric",
			month: "short",
			day: "numeric",
			hour: "2-digit",
			minute: "2-digit",
		});
	} catch {
		return "N/A";
	}
};

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
	const rootData = rootResult?.data;
	const instanceData = instanceResult?.data;

	if (isLoading)
		return <StatusCard icon={Server} text="Loading Instance Info..." />;
	if (error) return <StatusCard icon={AlertTriangle} text={error} isError />;

	return (
		<div className="w-full space-y-6">
			<div className="relative overflow-hidden rounded-2xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] p-8 shadow-lg">
				<div className="absolute right-0 top-0 h-48 w-48 translate-x-16 -translate-y-16 rounded-full bg-[var(--color-theme-bg)] blur-3xl" />
				<div className="relative">
					<div className="mb-6 flex flex-wrap items-start justify-between gap-4">
						<div className="flex items-center gap-5">
							{/* Logo with inline rotation animation */}
							<div className="flex h-20 w-20 items-center justify-center p-2">
								<img
									src={VaneLogo}
									alt="Vane Logo"
									className="h-full w-full"
									style={{
										animation: "spin 15s linear infinite",
									}}
								/>
							</div>
							<div>
								<h1 className="text-3xl font-bold text-[var(--color-text)]">
									Vane Engine
								</h1>
								<div className="mt-1 max-w-lg text-sm text-[var(--color-subtext)]">
									<p>Flow-based. Event-driven. Rust-native.</p>
									<p className="italic">
										Like a dandelion carried by the wind, it follows direction
										yet defines its own.
									</p>
								</div>
							</div>
						</div>
						{isActive && (
							<div className="flex flex-shrink-0 items-center gap-2 rounded-full border-2 border-[var(--color-theme-border)] bg-[var(--color-theme-bg)] px-4 py-2 text-sm font-semibold text-[var(--color-text)]">
								<Activity
									size={16}
									className="stroke-[var(--color-theme-border)]"
								/>
								Running
							</div>
						)}
					</div>
					<div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4">
						<StatCard
							icon={Package}
							label="Version"
							value={parseVersion(rootData?.build.vane)}
							accent
						/>
						<StatCard
							icon={GitCommitHorizontal}
							label="Build Hash"
							value={parseBuildHash(rootData?.build.vane)}
						/>
						<StatCard
							icon={Info}
							label="Instance ID"
							value={
								instanceData?.instance_id.slice(0, 8).toUpperCase() || "N/A"
							}
						/>
						<StatCard
							icon={Calendar}
							label="Created"
							value={formatDate(instanceData?.created_at || "")}
						/>
					</div>
				</div>
			</div>

			<div className="grid grid-cols-1 gap-6 lg:grid-cols-2">
				<DetailCard
					title="Runtime Environment"
					icon={Layers}
					items={[
						{
							icon: Cpu,
							label: "Platform",
							value: `${rootData?.runtime.platform} (${rootData?.runtime.arch})`,
						},
						{
							icon: Code,
							label: "Rust",
							value: parseToolVersion(rootData?.build.rust),
						},
						{
							icon: Terminal,
							label: "Cargo",
							value: parseToolVersion(rootData?.build.cargo),
						},
						{
							icon: Clock,
							label: "Server Time",
							value: formatDateTime(rootData?.timestamp || ""),
						},
					]}
				/>
				<DetailCard
					title="Open Source"
					icon={Github}
					items={[
						{
							icon: BookMarked,
							label: "Author",
							value: "https://github.com/canmi21",
							isLink: true,
							displayValue: "Canmi",
						},
						{
							icon: Scale,
							label: "License",
							value: rootData?.package.license || "N/A",
						},
						{
							icon: Github,
							label: "Repository",
							value: rootData?.package.repository || "N/A",
							isLink: true,
						},
						{
							icon: MessagesSquare,
							label: "Feedback",
							value: "https://github.com/canmi21/vane/issues",
							isLink: true,
						},
					]}
				/>
			</div>
		</div>
	);
}

// --- Reusable UI Components ---

function StatCard({
	icon: Icon,
	label,
	value,
	accent = false,
}: {
	icon: React.ElementType;
	label: string;
	value: string;
	accent?: boolean;
}) {
	return (
		<div
			className={`flex min-h-[72px] items-center rounded-xl border p-4 transition-all hover:shadow-md ${accent ? "border-[var(--color-theme-border)] bg-[var(--color-theme-bg)]" : "border-[var(--color-bg-alt)] bg-[var(--color-bg-alt)]"}`}
		>
			<div className="flex items-center gap-4">
				<div className="flex h-full items-center">
					<Icon
						size={28}
						className={
							accent
								? "stroke-[var(--color-theme-border)]"
								: "stroke-[var(--color-subtext)]"
						}
					/>
				</div>
				<div className="flex flex-col">
					<span className="text-xs text-[var(--color-subtext)]">{label}</span>
					<span className="font-mono text-base font-semibold text-[var(--color-text)]">
						{value}
					</span>
				</div>
			</div>
		</div>
	);
}

function DetailCard({
	title,
	icon: Icon,
	items,
}: {
	title: string;
	icon: React.ElementType;
	items: Array<{
		icon: React.ElementType;
		label: string;
		value: string;
		isLink?: boolean;
		displayValue?: string;
	}>;
}) {
	return (
		<div className="rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] p-6 shadow-sm transition-all hover:shadow-md">
			<div className="mb-4 flex items-center gap-3">
				<Icon size={20} className="stroke-[var(--color-theme-border)]" />
				<h3 className="text-lg font-semibold text-[var(--color-text)]">
					{title}
				</h3>
			</div>
			<div className="space-y-3">
				{items.map((item, idx) => (
					<div key={idx} className="flex min-h-[36px] items-center gap-4">
						<div className="flex h-full items-center">
							<item.icon
								size={20}
								className="flex-shrink-0 stroke-[var(--color-subtext)]"
							/>
						</div>
						<div className="flex flex-col">
							<span className="text-xs text-[var(--color-subtext)]">
								{item.label}
							</span>
							{item.isLink ? (
								<a
									href={item.value}
									target="_blank"
									rel="noopener noreferrer"
									className="break-all font-mono text-sm font-medium text-[var(--color-theme-border)] hover:underline"
								>
									{item.displayValue || item.value.replace(/^https?:\/\//, "")}
								</a>
							) : (
								<span className="font-mono text-sm font-medium text-[var(--color-text)]">
									{item.value}
								</span>
							)}
						</div>
					</div>
				))}
			</div>
		</div>
	);
}

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
