/* src/components/origins/origin-item.tsx */

import {
	Lock,
	Unlock,
	ShieldOff,
	ShieldCheck,
	Info,
	Pencil,
	Save,
	X,
	Trash2,
} from "lucide-react";
import { useState } from "react";
import * as Tooltip from "@radix-ui/react-tooltip";
import { type UseMutationResult } from "@tanstack/react-query";
import { type RequestResult } from "~/api/request";
import {
	type OriginResponse,
	type UpdateOriginPayload,
} from "~/routes/$instance/origins/";

export function OriginItem({
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
					{/* Info button with Radix tooltip */}
					<Tooltip.Root>
						<Tooltip.Trigger asChild>
							<button className="rounded-md p-1.5 text-[var(--color-subtext)] transition-all hover:scale-110 hover:text-[var(--color-theme-border)]">
								<Info size={16} />
							</button>
						</Tooltip.Trigger>
						<Tooltip.Portal>
							<Tooltip.Content
								className="z-50 rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg)] p-3 shadow-lg"
								sideOffset={5}
							>
								<div className="space-y-2 text-xs">
									<div className="flex justify-between gap-4">
										<span className="text-[var(--color-subtext)]">Scheme:</span>
										<span className="font-mono font-medium text-[var(--color-text)]">
											{item.scheme}
										</span>
									</div>
									<div className="flex justify-between gap-4">
										<span className="text-[var(--color-subtext)]">Host:</span>
										<span className="font-mono font-medium text-[var(--color-text)]">
											{item.host}
										</span>
									</div>
									<div className="flex justify-between gap-4">
										<span className="text-[var(--color-subtext)]">Port:</span>
										<span className="font-mono font-medium text-[var(--color-text)]">
											{item.port}
										</span>
									</div>
									<div className="flex justify-between gap-4">
										<span className="text-[var(--color-subtext)]">Path:</span>
										<span className="font-mono font-medium text-[var(--color-text)]">
											{item.path}
										</span>
									</div>
									<div className="flex justify-between gap-4">
										<span className="text-[var(--color-subtext)]">
											SSL Verify:
										</span>
										<span className="font-mono font-medium text-[var(--color-text)]">
											{item.skip_ssl_verify ? "false" : "true"}
										</span>
									</div>
								</div>
								<Tooltip.Arrow className="fill-[var(--color-bg-alt)]" />
							</Tooltip.Content>
						</Tooltip.Portal>
					</Tooltip.Root>
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
