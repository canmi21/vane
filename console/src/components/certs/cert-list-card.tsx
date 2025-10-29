/* src/components/certs/cert-list-card.tsx */

import { useState } from "react";
import {
	BadgeCheck,
	BadgeHelp,
	Plus,
	X,
	UploadCloud,
	Trash2,
	ChevronRight,
} from "lucide-react";
import { motion, AnimatePresence } from "framer-motion";
import { type UseMutationResult } from "@tanstack/react-query";
import { type RequestResult } from "~/api/request";
import {
	type CertSummary,
	type CertInfo,
} from "~/routes/$instance/certificates/$domain";

// --- New combined type for list items ---
export type CertListItem = {
	domain: string;
	summary: CertSummary;
	details: CertInfo | null;
};

// --- Helper to format expiry date relatively ---
function formatExpiry(dateString: string): {
	text: string;
	isSoon: boolean;
	isExpired: boolean;
} {
	const now = new Date();
	const expiryDate = new Date(dateString);
	now.setHours(0, 0, 0, 0);
	expiryDate.setHours(0, 0, 0, 0);

	const diffTime = expiryDate.getTime() - now.getTime();
	const diffDays = Math.ceil(diffTime / (1000 * 60 * 60 * 24));

	if (diffDays < 0) {
		return {
			text: `-${Math.abs(diffDays)} days`,
			isSoon: false,
			isExpired: true,
		};
	}
	if (diffDays === 0) {
		return { text: "Expires today", isSoon: true, isExpired: false };
	}
	if (diffDays > 365) {
		const years = Math.floor(diffDays / 365);
		return {
			text: `in ~${years} year(s)`,
			isSoon: false,
			isExpired: false,
		};
	}
	return {
		text: `in ${diffDays} day(s)`,
		isSoon: diffDays <= 30, // Highlight if expiring within 30 days
		isExpired: false,
	};
}

// --- Certificate List Item Component ---
function CertItem({
	item,
	isSelected,
	onSelect,
	onDelete,
	isDeleting,
}: {
	item: CertListItem;
	isSelected: boolean;
	onSelect: () => void;
	onDelete: () => void;
	isDeleting: boolean;
}) {
	const expiryInfo = formatExpiry(item.summary.expires_at);
	let subtextColor = "text-[var(--color-subtext)]";
	if (expiryInfo.isExpired) {
		subtextColor = "text-red-500";
	} else if (expiryInfo.isSoon) {
		subtextColor = "text-orange-500";
	}

	const commonName = item.details?.subject.common_name;

	const isSelfSigned = item.details
		? item.details.subject.raw_string === item.details.issuer.raw_string
		: false;
	const IconComponent = isSelfSigned ? BadgeHelp : BadgeCheck;
	const iconTitle = isSelfSigned ? "Self-Signed" : "CA Signed";

	return (
		<div
			onClick={onSelect}
			className={`flex cursor-pointer items-center justify-between p-4 transition-all hover:bg-[var(--color-theme-bg)] ${
				isSelected ? "bg-[var(--color-theme-bg)]" : ""
			}`}
		>
			<div className="flex min-w-0 items-center gap-4">
				<div className="flex-shrink-0" title={iconTitle}>
					<IconComponent
						size={20}
						className={
							isSelected
								? "stroke-[var(--color-theme-border)]"
								: "stroke-[var(--color-subtext)]"
						}
					/>
				</div>
				<div className="min-w-0">
					<p className="truncate font-mono text-sm font-medium text-[var(--color-text)]">
						{item.domain}
					</p>
					<p className={`truncate text-xs ${subtextColor}`}>
						{commonName ? `CN: ${commonName} • ` : ""}
						{expiryInfo.isExpired
							? `Expired (${expiryInfo.text})`
							: expiryInfo.text}
					</p>
				</div>
			</div>
			<div className="flex flex-shrink-0 items-center gap-2">
				<button
					onClick={(e) => {
						e.stopPropagation();
						onDelete();
					}}
					disabled={isDeleting}
					className="rounded-md p-2 text-[var(--color-subtext)] transition-colors hover:text-red-500 disabled:opacity-50"
					title={`Delete ${item.domain}`}
				>
					<Trash2 size={16} />
				</button>
				<ChevronRight
					size={18}
					className={`transition-transform ${
						isSelected ? "translate-x-1" : ""
					}`}
				/>
			</div>
		</div>
	);
}

// --- Main Card Component ---
export function CertListCard({
	certs,
	selectedDomain,
	onSelectDomain,
	uploadMutation,
	removeMutation,
}: {
	certs: CertListItem[];
	selectedDomain: string | null;
	onSelectDomain: (domain: string | null) => void;
	uploadMutation: UseMutationResult<
		RequestResult<unknown>,
		Error,
		{ domain: string; certPem: string; keyPem: string }
	>;
	removeMutation: UseMutationResult<RequestResult<unknown>, Error, string>;
}) {
	const [isAdding, setIsAdding] = useState(false);
	const [domain, setDomain] = useState("");
	const [certPem, setCertPem] = useState("");
	const [keyPem, setKeyPem] = useState("");

	const handleUpload = (e: React.FormEvent) => {
		e.preventDefault();
		if (domain.trim() && certPem.trim() && keyPem.trim()) {
			uploadMutation.mutate(
				{
					domain: domain.trim(),
					certPem: certPem.trim(),
					keyPem: keyPem.trim(),
				},
				{
					onSuccess: () => {
						setIsAdding(false);
						setDomain("");
						setCertPem("");
						setKeyPem("");
					},
				}
			);
		}
	};

	const handleDelete = (domainToDelete: string) => {
		if (
			window.confirm(
				`Are you sure you want to delete the certificate for "${domainToDelete}"?`
			)
		) {
			removeMutation.mutate(domainToDelete);
		}
	};

	return (
		<div className="w-full rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-sm">
			{/* Card Header */}
			<div className="border-b border-[var(--color-bg-alt)] p-6">
				<div className="flex items-center justify-between">
					<div className="flex items-center gap-3">
						<BadgeCheck
							size={20}
							className="stroke-[var(--color-theme-border)]"
						/>
						<h3 className="text-lg font-semibold text-[var(--color-text)]">
							SSL/TLS Certificates
						</h3>
						<span className="rounded-md bg-[var(--color-bg-alt)] px-2 py-0.5 text-xs font-medium text-[var(--color-subtext)]">
							{certs.length}
						</span>
					</div>
					<button
						onClick={() => setIsAdding(!isAdding)}
						className="flex h-10 items-center gap-2 rounded-lg border-2 border-[var(--color-theme-border)] bg-[var(--color-theme-bg)] px-3 text-sm font-semibold text-[var(--color-text)] transition-all hover:opacity-80"
					>
						{isAdding ? (
							<>
								<X size={16} /> Cancel
							</>
						) : (
							<>
								<Plus size={16} /> Add Certificate
							</>
						)}
					</button>
				</div>
			</div>

			{/* Collapsible Upload Form */}
			<AnimatePresence>
				{isAdding && (
					<motion.div
						initial={{ height: 0, opacity: 0 }}
						animate={{ height: "auto", opacity: 1 }}
						exit={{ height: 0, opacity: 0 }}
						transition={{ duration: 0.3, ease: "easeInOut" }}
						className="overflow-hidden border-b border-[var(--color-bg-alt)]"
					>
						<form onSubmit={handleUpload} className="space-y-4 p-6">
							<input
								type="text"
								value={domain}
								onChange={(e) => setDomain(e.target.value)}
								placeholder="Domain (e.g., example.com or *.example.com)"
								className="h-10 w-full rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg-alt)] px-3 text-sm text-[var(--color-text)] placeholder-[var(--color-subtext)] transition-all focus:border-[var(--color-theme-border)] focus:outline-none focus:ring-1 focus:ring-[var(--color-theme-border)]"
								disabled={uploadMutation.isPending}
								required
								autoFocus
							/>
							<div className="grid grid-cols-1 gap-4 md:grid-cols-2">
								<textarea
									value={certPem}
									onChange={(e) => setCertPem(e.target.value)}
									placeholder="Paste your certificate PEM here..."
									className="h-40 w-full resize-y rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg-alt)] p-3 font-mono text-xs text-[var(--color-text)] placeholder-[var(--color-subtext)] transition-all focus:border-[var(--color-theme-border)] focus:outline-none focus:ring-1 focus:ring-[var(--color-theme-border)]"
									disabled={uploadMutation.isPending}
									required
								/>
								<textarea
									value={keyPem}
									onChange={(e) => setKeyPem(e.target.value)}
									placeholder="Paste your private key PEM here..."
									className="h-40 w-full resize-y rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg-alt)] p-3 font-mono text-xs text-[var(--color-text)] placeholder-[var(--color-subtext)] transition-all focus:border-[var(--color-theme-border)] focus:outline-none focus:ring-1 focus:ring-[var(--color-theme-border)]"
									disabled={uploadMutation.isPending}
									required
								/>
							</div>
							<div className="flex items-center justify-end gap-3">
								{uploadMutation.isError && (
									<p className="text-xs text-red-500">
										{uploadMutation.error?.message || "Upload failed."}
									</p>
								)}
								<button
									type="submit"
									className="flex h-10 items-center gap-2 rounded-lg bg-[var(--color-theme-bg)] px-4 text-sm font-semibold text-[var(--color-text)] transition-all hover:opacity-80 disabled:cursor-not-allowed disabled:opacity-50"
									disabled={
										uploadMutation.isPending || !domain || !certPem || !keyPem
									}
								>
									<UploadCloud size={16} /> Upload and Save
								</button>
							</div>
						</form>
					</motion.div>
				)}
			</AnimatePresence>

			{/* Certificate List --- FIX: Added overflow-hidden and rounding --- */}
			<div className="overflow-hidden rounded-b-xl divide-y divide-[var(--color-bg-alt)]">
				{certs.length > 0 ? (
					certs.map((item) => (
						<CertItem
							key={item.domain}
							item={item}
							isSelected={selectedDomain === item.domain}
							onSelect={() => onSelectDomain(item.domain)}
							onDelete={() => handleDelete(item.domain)}
							isDeleting={
								removeMutation.isPending &&
								removeMutation.variables === item.domain
							}
						/>
					))
				) : (
					<div className="p-12 text-center text-[var(--color-subtext)]">
						<p className="font-medium">No certificates found.</p>
						<p className="text-sm">Click "Add Certificate" to upload one.</p>
					</div>
				)}
			</div>
		</div>
	);
}
