/* src/routes/$instance/origins/index.tsx */

import { createFileRoute, useParams } from "@tanstack/react-router";
import {
	useQuery,
	useMutation,
	useQueryClient,
	type UseMutationResult,
} from "@tanstack/react-query";
import {
	Server,
	Plus,
	Trash2,
	Save,
	X,
	Pencil,
	ServerCrash,
	Globe,
	Lock,
	Unlock,
	ShieldCheck,
	ShieldOff,
	ShieldAlert,
	Link as LinkIcon,
	Info,
	Target,
} from "lucide-react";
import React, { useState, useMemo } from "react";
import { motion, AnimatePresence } from "framer-motion";
import { type RequestResult } from "~/api/request";
import {
	getInstance,
	postInstance,
	putInstance,
	deleteInstance,
} from "~/api/instance";

// --- API Helper Functions ---
async function createOrigin(
	instanceId: string,
	url: string
): Promise<RequestResult<OriginResponse>> {
	return postInstance<OriginResponse>(instanceId, "/v1/origins", { url });
}

type UpdateOriginPayload = { raw_url?: string; skip_ssl_verify?: boolean };

async function updateOrigin(
	instanceId: string,
	originId: string,
	payload: UpdateOriginPayload
): Promise<RequestResult<OriginResponse>> {
	return putInstance<OriginResponse>(
		instanceId,
		`/v1/origins/${originId}`,
		payload
	);
}

async function deleteOrigin(
	instanceId: string,
	originId: string
): Promise<RequestResult<unknown>> {
	return deleteInstance<unknown>(instanceId, `/v1/origins/${originId}`);
}

// --- Data Types ---
interface OriginResponse {
	id: string;
	scheme: "http" | "https";
	host: string;
	port: number;
	path: string;
	skip_ssl_verify: boolean;
	raw_url: string;
}

export const Route = createFileRoute("/$instance/origins/")({
	component: OriginsPage,
});

// --- Main Page Component ---
function OriginsPage() {
	const { instance: instanceId } = useParams({ from: "/$instance/origins/" });
	const queryClient = useQueryClient();

	const {
		data: originsResult,
		isLoading,
		isError,
		error,
	} = useQuery<RequestResult<OriginResponse[]>>({
		queryKey: ["instance", instanceId, "origins"],
		queryFn: () => getInstance(instanceId, "/v1/origins"),
	});

	const origins = useMemo(
		() => originsResult?.data ?? [],
		[originsResult?.data]
	);

	const stats = useMemo(() => {
		const httpCount = origins.filter((o) => o.scheme === "http").length;
		const httpsCount = origins.filter((o) => o.scheme === "https").length;
		const sslSkipped = origins.filter((o) => o.skip_ssl_verify).length;
		return {
			total: origins.length,
			http: httpCount,
			https: httpsCount,
			sslSkipped,
		};
	}, [origins]);

	const addMutation = useMutation<RequestResult<OriginResponse>, Error, string>(
		{
			mutationFn: (newUrl) => createOrigin(instanceId, newUrl),
			onSuccess: () => {
				queryClient.invalidateQueries({
					queryKey: ["instance", instanceId, "origins"],
				});
			},
		}
	);

	const updateMutation = useMutation<
		RequestResult<OriginResponse>,
		Error,
		{ id: string; payload: UpdateOriginPayload }
	>({
		mutationFn: (variables) =>
			updateOrigin(instanceId, variables.id, variables.payload),
		onSuccess: () => {
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "origins"],
			});
		},
	});

	const removeMutation = useMutation<RequestResult<unknown>, Error, string>({
		mutationFn: (id) => deleteOrigin(instanceId, id),
		onSuccess: () => {
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "origins"],
			});
		},
	});

	if (isLoading) {
		return <StatusCard icon={Server} text="Loading Origins..." />;
	}
	if (isError) {
		return (
			<StatusCard
				icon={ServerCrash}
				text={error?.message || "Failed to fetch origins."}
				isError
			/>
		);
	}

	return (
		<div className="space-y-6">
			<SummaryCard stats={stats} />
			<OriginListCard
				origins={origins}
				addMutation={addMutation}
				updateMutation={updateMutation}
				removeMutation={removeMutation}
			/>
		</div>
	);
}

// --- UI Components ---
function SummaryCard({
	stats,
}: {
	stats: { total: number; http: number; https: number; sslSkipped: number };
}) {
	return (
		<div className="rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] p-6 shadow-sm">
			<div className="mb-4 flex items-center gap-3">
				<Target size={20} className="stroke-[var(--color-theme-border)]" />
				<h3 className="text-lg font-semibold text-[var(--color-text)]">
					Origins Overview
				</h3>
			</div>
			<div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
				<SummaryStatCard
					icon={Server}
					label="Total Origins"
					value={stats.total}
				/>
				<SummaryStatCard icon={Lock} label="HTTPS" value={stats.https} />
				<SummaryStatCard icon={Unlock} label="HTTP" value={stats.http} />
				<SummaryStatCard
					icon={ShieldAlert}
					label="SSL Skipped"
					value={stats.sslSkipped}
				/>
			</div>
		</div>
	);
}

function SummaryStatCard({
	icon: Icon,
	label,
	value,
}: {
	icon: React.ElementType;
	label: string;
	value: number;
}) {
	return (
		<div className="flex items-center gap-4 rounded-lg bg-[var(--color-bg-alt)] p-4">
			<Icon size={40} className="stroke-[var(--color-subtext)]" />
			<div>
				<div className="text-xs text-[var(--color-subtext)]">{label}</div>
				<div className="text-xl font-bold text-[var(--color-text)]">
					{value}
				</div>
			</div>
		</div>
	);
}

// --- OriginListCard ---
function OriginListCard({
	origins,
	addMutation,
	updateMutation,
	removeMutation,
}: {
	origins: OriginResponse[];
	addMutation: UseMutationResult<RequestResult<OriginResponse>, Error, string>;
	updateMutation: UseMutationResult<
		RequestResult<OriginResponse>,
		Error,
		{ id: string; payload: UpdateOriginPayload }
	>;
	removeMutation: UseMutationResult<RequestResult<unknown>, Error, string>;
}) {
	const [newOriginUrl, setNewOriginUrl] = useState("");
	const [isAddFormExpanded, setIsAddFormExpanded] = useState(false);

	const handleAddOrigin = (e: React.FormEvent) => {
		e.preventDefault();
		if (newOriginUrl.trim()) {
			addMutation.mutate(newOriginUrl.trim(), {
				onSuccess: () => setNewOriginUrl(""),
			});
		}
	};

	return (
		<div className="rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-sm">
			{/* Header with title */}
			<div className="border-b border-[var(--color-bg-alt)] p-6">
				<div className="flex items-center justify-between">
					<div className="flex items-center gap-3">
						<Globe size={20} className="stroke-[var(--color-theme-border)]" />
						<h3 className="text-lg font-semibold text-[var(--color-text)]">
							Configured Origins
						</h3>
						<span className="rounded-md bg-[var(--color-bg-alt)] px-2 py-0.5 text-xs font-medium text-[var(--color-subtext)]">
							{origins.length}
						</span>
					</div>
					{/* Toggle add form button */}
					<button
						onClick={() => setIsAddFormExpanded(!isAddFormExpanded)}
						className="flex items-center gap-2 rounded-lg border-2 border-[var(--color-theme-border)] bg-[var(--color-theme-bg)] px-3 py-1.5 text-sm font-semibold text-[var(--color-text)] transition-all hover:opacity-80"
					>
						{isAddFormExpanded ? (
							<>
								<X size={16} />
								Cancel
							</>
						) : (
							<>
								<Plus size={16} />
								Add Origin
							</>
						)}
					</button>
				</div>
			</div>

			{/* Collapsible add form */}
			<AnimatePresence>
				{isAddFormExpanded && (
					<motion.div
						initial={{ height: 0, opacity: 0 }}
						animate={{ height: "auto", opacity: 1 }}
						exit={{ height: 0, opacity: 0 }}
						transition={{ duration: 0.3, ease: "easeInOut" }}
						className="overflow-hidden border-b border-[var(--color-bg-alt)]"
					>
						<div className="p-4">
							<form onSubmit={handleAddOrigin} className="flex gap-2">
								<input
									type="text"
									value={newOriginUrl}
									onChange={(e) => setNewOriginUrl(e.target.value)}
									placeholder="http(s)://{host}:{port}/{path}"
									className="flex-grow rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg-alt)] px-3 py-2 text-sm text-[var(--color-text)] placeholder-[var(--color-subtext)] transition-all focus:border-[var(--color-theme-border)] focus:outline-none focus:ring-1 focus:ring-[var(--color-theme-border)]"
									disabled={addMutation.isPending}
									autoFocus
								/>
								<button
									type="submit"
									className="flex items-center gap-2 rounded-lg border-2 border-[var(--color-theme-border)] bg-[var(--color-theme-bg)] px-4 py-2 text-sm font-semibold text-[var(--color-text)] transition-all hover:opacity-80 disabled:cursor-not-allowed disabled:opacity-50"
									disabled={addMutation.isPending || !newOriginUrl.trim()}
								>
									<Save size={16} />
									Save
								</button>
							</form>
							{addMutation.isError && (
								<p className="mt-2 text-xs text-red-500">
									{addMutation.error?.message || "Failed to add origin."}
								</p>
							)}
						</div>
					</motion.div>
				)}
			</AnimatePresence>

			{/* Origins list - always visible */}
			<div className="divide-y divide-[var(--color-bg-alt)]">
				{origins.length > 0 ? (
					origins.map((item) => (
						<OriginItem
							key={item.id}
							item={item}
							updateMutation={updateMutation}
							removeMutation={removeMutation}
						/>
					))
				) : (
					<div className="flex flex-col items-center gap-4 p-12 text-center text-[var(--color-subtext)]">
						<LinkIcon size={32} />
						<p className="font-medium">No origins configured.</p>
						<p className="text-sm">Click "Add Origin" above to get started.</p>
					</div>
				)}
			</div>
		</div>
	);
}

// --- OriginItem ---
function OriginItem({
	item,
	updateMutation,
	removeMutation,
}: {
	item: OriginResponse;
	updateMutation: UseMutationResult<
		RequestResult<OriginResponse>,
		Error,
		{ id: string; payload: UpdateOriginPayload }
	>;
	removeMutation: UseMutationResult<RequestResult<unknown>, Error, string>;
}) {
	const [isEditing, setIsEditing] = useState(false);
	const [editedUrl, setEditedUrl] = useState(item.raw_url);
	const [skipVerify, setSkipVerify] = useState(item.skip_ssl_verify);

	const handleSave = () => {
		const payload: UpdateOriginPayload = {};
		if (editedUrl.trim() !== item.raw_url) {
			payload.raw_url = editedUrl.trim();
		}
		if (skipVerify !== item.skip_ssl_verify) {
			payload.skip_ssl_verify = skipVerify;
		}
		if (Object.keys(payload).length > 0) {
			updateMutation.mutate(
				{ id: item.id, payload },
				{ onSuccess: () => setIsEditing(false) }
			);
		} else {
			setIsEditing(false);
		}
	};

	const handleCancel = () => {
		setEditedUrl(item.raw_url);
		setSkipVerify(item.skip_ssl_verify);
		setIsEditing(false);
	};

	const handleDelete = () => {
		if (window.confirm(`Are you sure you want to delete "${item.id}"?`)) {
			removeMutation.mutate(item.id);
		}
	};

	const isHttps = item.scheme === "https";

	if (isEditing) {
		return (
			<div className="bg-[var(--color-theme-bg)] p-3">
				<div className="flex items-center gap-3">
					<input
						type="text"
						value={editedUrl}
						onChange={(e) => setEditedUrl(e.target.value)}
						className="flex-grow rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg-alt)] px-3 py-1.5 font-mono text-sm text-[var(--color-text)] placeholder-[var(--color-subtext)] transition-all focus:border-[var(--color-theme-border)] focus:outline-none focus:ring-1 focus:ring-[var(--color-theme-border)]"
					/>
					<button
						onClick={handleSave}
						disabled={updateMutation.isPending}
						className="rounded-md p-1.5 text-[var(--color-subtext)] hover:text-[var(--color-theme-border)] disabled:opacity-50"
						title="Save changes"
					>
						<Save size={18} />
					</button>
					<button
						onClick={handleCancel}
						disabled={updateMutation.isPending}
						className="rounded-md p-1.5 text-[var(--color-subtext)] hover:text-[var(--color-text)]"
						title="Cancel"
					>
						<X size={18} />
					</button>
				</div>
				{isHttps && (
					<div className="mt-2">
						<label className="flex cursor-pointer items-center gap-2 text-sm text-[var(--color-text)]">
							<input
								type="checkbox"
								checked={skipVerify}
								onChange={(e) => setSkipVerify(e.target.checked)}
								className="h-4 w-4 rounded border-[var(--color-tertiary)] accent-[var(--color-theme-border)]"
							/>
							Skip SSL certificate verification
						</label>
						<p className="ml-6 text-xs text-[var(--color-subtext)]">
							Not recommended. Only use for self-signed certificates.
						</p>
					</div>
				)}
				{updateMutation.isError && (
					<p className="mt-2 text-xs text-red-500">
						{updateMutation.error?.message || "Failed to update."}
					</p>
				)}
			</div>
		);
	}

	return (
		<div className="px-4 py-2.5 transition-all hover:bg-[var(--color-theme-bg)]">
			<div className="flex items-center gap-3">
				{/* URL and basic info - takes most space */}
				<div className="flex min-w-0 flex-grow items-center gap-3">
					<span className="truncate font-mono text-sm font-medium text-[var(--color-text)]">
						{item.raw_url}
					</span>
					<span className="flex-shrink-0 rounded-md bg-[var(--color-bg-alt)] px-2 py-0.5 font-mono text-xs text-[var(--color-subtext)]">
						{item.id}
					</span>
				</div>

				{/* Compact info badges */}
				<div className="flex flex-shrink-0 items-center gap-2">
					{/* Scheme */}
					{isHttps ? (
						<div className="flex items-center gap-1 text-xs">
							<Lock size={14} className="stroke-[var(--color-subtext)]" />
						</div>
					) : (
						<div className="flex items-center gap-1 text-xs">
							<Unlock size={14} className="stroke-[var(--color-subtext)]" />
						</div>
					)}

					{/* SSL status for HTTPS */}
					{isHttps && (
						<div
							className="flex items-center"
							title={
								item.skip_ssl_verify
									? "SSL Verification Skipped"
									: "SSL Verified"
							}
						>
							{item.skip_ssl_verify ? (
								<ShieldOff
									size={14}
									className="stroke-[var(--color-subtext)]"
								/>
							) : (
								<ShieldCheck
									size={14}
									className="stroke-[var(--color-subtext)]"
								/>
							)}
						</div>
					)}

					{/* Host:Port */}
					<span className="font-mono text-xs text-[var(--color-subtext)]">
						{item.host}:{item.port}
					</span>

					{/* Path if not root */}
					{item.path !== "/" && (
						<span className="font-mono text-xs text-[var(--color-subtext)]">
							{item.path}
						</span>
					)}
				</div>

				{/* Action buttons */}
				<div className="flex flex-shrink-0 items-center gap-1">
					{/* Info button with tooltip */}
					<div className="group relative">
						<button
							className="rounded-md p-1.5 text-[var(--color-subtext)] transition-all hover:scale-110 hover:text-[var(--color-theme-border)]"
							title="View parsed details"
						>
							<Info size={16} />
						</button>
						{/* Tooltip */}
						<div className="pointer-events-none absolute right-0 top-full z-10 mt-2 w-64 opacity-0 transition-opacity group-hover:pointer-events-auto group-hover:opacity-100">
							<div className="rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg)] p-3 shadow-lg">
								<div className="space-y-2 text-xs">
									<div className="flex justify-between">
										<span className="text-[var(--color-subtext)]">Scheme:</span>
										<span className="font-mono font-medium text-[var(--color-text)]">
											{item.scheme}
										</span>
									</div>
									<div className="flex justify-between">
										<span className="text-[var(--color-subtext)]">Host:</span>
										<span className="font-mono font-medium text-[var(--color-text)]">
											{item.host}
										</span>
									</div>
									<div className="flex justify-between">
										<span className="text-[var(--color-subtext)]">Port:</span>
										<span className="font-mono font-medium text-[var(--color-text)]">
											{item.port}
										</span>
									</div>
									<div className="flex justify-between">
										<span className="text-[var(--color-subtext)]">Path:</span>
										<span className="font-mono font-medium text-[var(--color-text)]">
											{item.path}
										</span>
									</div>
									<div className="flex justify-between">
										<span className="text-[var(--color-subtext)]">
											SSL Verify:
										</span>
										<span className="font-mono font-medium text-[var(--color-text)]">
											{item.skip_ssl_verify ? "false" : "true"}
										</span>
									</div>
								</div>
							</div>
						</div>
					</div>
					<button
						onClick={() => setIsEditing(true)}
						className="rounded-md p-1.5 text-[var(--color-subtext)] transition-all hover:scale-110 hover:text-[var(--color-theme-border)]"
						title="Edit origin"
					>
						<Pencil size={16} />
					</button>
					<button
						onClick={handleDelete}
						disabled={removeMutation.isPending}
						className="rounded-md p-1.5 text-[var(--color-subtext)] transition-all hover:scale-110 hover:text-[var(--color-text)] disabled:opacity-50"
						title="Delete origin"
					>
						<Trash2 size={16} />
					</button>
				</div>
			</div>
		</div>
	);
}

// --- InfoItem and StatusCard ---
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
