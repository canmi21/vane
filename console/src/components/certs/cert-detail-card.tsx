/* src/components/certs/cert-detail-card.tsx */

import { Info, CheckCircle, XCircle, Loader2 } from "lucide-react";
import { type UseQueryResult } from "@tanstack/react-query";
import { type RequestResult } from "~/api/request";
import { type CertInfo, type NameInfo } from "~/routes/$instance/certificates/";

// --- Helper component for displaying a key-value pair ---
function DetailRow({
	label,
	children,
	isMono = false,
}: {
	label: string;
	children: React.ReactNode;
	isMono?: boolean;
}) {
	return (
		<div className="flex flex-col gap-1 sm:flex-row sm:justify-between sm:gap-4">
			<span className="flex-shrink-0 text-sm text-[var(--color-subtext)]">
				{label}
			</span>
			<span
				className={`break-words text-right text-sm font-medium text-[var(--color-text)] ${
					isMono ? "font-mono" : ""
				}`}
			>
				{children}
			</span>
		</div>
	);
}

// --- Helper component for displaying Subject/Issuer info ---
function NameDetail({ title, data }: { title: string; data: NameInfo }) {
	return (
		<div className="space-y-2 rounded-lg bg-[var(--color-bg-alt)] p-4">
			<h4 className="font-semibold text-[var(--color-text)]">{title}</h4>
			{data.common_name && (
				<DetailRow label="Common Name (CN)">{data.common_name}</DetailRow>
			)}
			{data.organization && (
				<DetailRow label="Organization (O)">{data.organization}</DetailRow>
			)}
			{data.organizational_unit && (
				<DetailRow label="Org. Unit (OU)">{data.organizational_unit}</DetailRow>
			)}
			{data.country && (
				<DetailRow label="Country (C)">{data.country}</DetailRow>
			)}
		</div>
	);
}

// --- Main Detail Card Component ---
export function CertDetailCard({
	query,
}: {
	query: UseQueryResult<RequestResult<CertInfo>>;
}) {
	const { data: result, isLoading, isError, error } = query;
	const cert = result?.data;

	const renderContent = () => {
		if (isLoading) {
			return (
				<div className="flex items-center justify-center gap-3 p-12">
					<Loader2
						size={24}
						className="animate-spin text-[var(--color-subtext)]"
					/>
					<p className="text-[var(--color-subtext)]">Loading details...</p>
				</div>
			);
		}
		if (isError) {
			return (
				<div className="p-6 text-center text-red-500">
					{error?.message || "Failed to load certificate details."}
				</div>
			);
		}
		if (!cert) {
			return (
				<div className="p-6 text-center text-[var(--color-subtext)]">
					No details available.
				</div>
			);
		}

		return (
			<div className="space-y-6 p-6">
				{/* Validity Section */}
				<div className="space-y-2">
					<DetailRow label="Validity Status">
						{cert.validity.is_valid ? (
							<span className="flex items-center justify-end gap-2 text-green-600">
								<CheckCircle size={16} /> Valid
							</span>
						) : (
							<span className="flex items-center justify-end gap-2 text-red-500">
								<XCircle size={16} /> Expired or Invalid
							</span>
						)}
					</DetailRow>
					<DetailRow label="Not Valid Before">
						{new Date(cert.validity.not_before).toLocaleString()}
					</DetailRow>
					<DetailRow label="Not Valid After">
						{new Date(cert.validity.not_after).toLocaleString()}
					</DetailRow>
				</div>

				{/* Names Grid */}
				<div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
					<NameDetail title="Subject" data={cert.subject} />
					<NameDetail title="Issuer" data={cert.issuer} />
				</div>

				{/* Technical Details Section */}
				<div className="space-y-2">
					<DetailRow label="Subject Alternative Names (SANs)">
						<div className="flex flex-col items-end">
							{cert.subject_alternative_names.map((san) => (
								<span key={san}>{san}</span>
							))}
						</div>
					</DetailRow>
					<DetailRow label="Serial Number" isMono>
						{cert.serial_number}
					</DetailRow>
					<DetailRow label="Public Key">
						{cert.public_key.algorithm} ({cert.public_key.key_size_bits} bits)
					</DetailRow>
					<DetailRow label="Signature Algorithm">
						{cert.signature.algorithm}
					</DetailRow>
					<DetailRow label="Fingerprint (SHA-256)" isMono>
						{cert.fingerprints.sha256}
					</DetailRow>
					<DetailRow label="Fingerprint (SHA-1)" isMono>
						{cert.fingerprints.sha1}
					</DetailRow>
				</div>
			</div>
		);
	};

	return (
		<div className="w-full rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-sm">
			{/* Card Header */}
			<div className="border-b border-[var(--color-bg-alt)] p-6">
				<div className="flex items-center gap-3">
					<Info size={20} className="stroke-[var(--color-theme-border)]" />
					<h3 className="text-lg font-semibold text-[var(--color-text)]">
						Certificate Details
					</h3>
				</div>
			</div>
			{renderContent()}
		</div>
	);
}
