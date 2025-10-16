/* src/routes/$instance/certificates/index.tsx */

import {
	createFileRoute,
	useParams,
	useNavigate,
	useLocation,
} from "@tanstack/react-router";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Server, ServerCrash } from "lucide-react";
import React, { useState, useEffect, useMemo, useCallback } from "react";
import { type RequestResult } from "~/api/request";
import { getInstance, postInstance, deleteInstance } from "~/api/instance";
import {
	CertOverviewCard,
	type CertOverviewStats,
} from "~/components/certs/cert-overview-card";
import {
	CertListCard,
	type CertListItem,
} from "~/components/certs/cert-list-card";
import { CertDetailCard } from "~/components/certs/cert-detail-card";
import * as Tooltip from "@radix-ui/react-tooltip";

// --- API Helper Functions ---
async function listCerts(
	instanceId: string
): Promise<RequestResult<ListCertsResponse>> {
	return getInstance(instanceId, "/v1/certs");
}

async function getCertDetails(
	instanceId: string,
	domain: string
): Promise<RequestResult<CertInfo>> {
	const encodedDomain = encodeURIComponent(domain);
	return getInstance(instanceId, `/v1/certs/${encodedDomain}`);
}

async function uploadCert(
	instanceId: string,
	domain: string,
	payload: { cert_pem_b64: string; key_pem_b64: string }
): Promise<RequestResult<unknown>> {
	const encodedDomain = encodeURIComponent(domain);
	return postInstance(instanceId, `/v1/certs/${encodedDomain}`, payload);
}

async function deleteCert(
	instanceId: string,
	domain: string
): Promise<RequestResult<unknown>> {
	const encodedDomain = encodeURIComponent(domain);
	return deleteInstance(instanceId, `/v1/certs/${encodedDomain}`);
}

// --- Data Types from Backend (interfaces are unchanged) ---
export interface CertSummary {
	filename: string;
	format: string;
	expires_at: string;
	issued_to: string[];
}

export interface ListCertsResponse {
	certificates: Record<string, CertSummary>;
}

export interface CertInfo {
	subject: NameInfo;
	issuer: NameInfo;
	validity: ValidityInfo;
	subject_alternative_names: string[];
	public_key: PublicKeyInfo;
	serial_number: string;
	signature: SignatureInfo;
	fingerprints: Fingerprints;
	is_ca: boolean;
}

export interface NameInfo {
	common_name: string | null;
	organization: string | null;
	organizational_unit: string | null;
	country: string | null;
	state: string | null;
	locality: string | null;
	raw_string: string;
}

export interface ValidityInfo {
	not_before: string;
	not_after: string;
	is_valid: boolean;
}

export interface PublicKeyInfo {
	algorithm: string;
	key_size_bits: number;
}
export interface SignatureInfo {
	algorithm: string;
	value: string;
}
export interface Fingerprints {
	sha1: string;
	sha256: string;
}

export const Route = createFileRoute("/$instance/certificates/")({
	component: CertificatesPage,
});

function CertificatesPage() {
	const { instance: instanceId } = useParams({
		from: "/$instance/certificates/",
	});
	const queryClient = useQueryClient();
	const navigate = useNavigate();
	const location = useLocation();

	const [selectedDomain, setSelectedDomain] = useState<string | null>(null);

	// --- Step 1: Query for the list of all certificate summaries ---
	const {
		data: certsResult,
		isLoading: isListLoading,
		isError: isListError,
		error: listError,
	} = useQuery<RequestResult<ListCertsResponse>>({
		queryKey: ["instance", instanceId, "certs", "list"],
		queryFn: () => listCerts(instanceId),
	});

	const certificates = useMemo(
		() => certsResult?.data?.certificates ?? {},
		[certsResult]
	);
	const certDomains = useMemo(() => Object.keys(certificates), [certificates]);

	// --- Step 2: Fetch details for ALL certificates in parallel ---
	const {
		data: allDetailsMap,
		isLoading: areDetailsLoading,
		isError: isDetailsError,
		error: detailsError,
	} = useQuery<Record<string, CertInfo>>({
		queryKey: ["instance", instanceId, "certs", "details", "all"],
		queryFn: async () => {
			const detailPromises = certDomains.map(async (domain) => {
				const result = await getCertDetails(instanceId, domain);
				return { domain, details: result.data };
			});
			const results = await Promise.all(detailPromises);
			const map: Record<string, CertInfo> = {};
			for (const { domain, details } of results) {
				if (details) {
					map[domain] = details;
				}
			}
			return map;
		},
		enabled: certDomains.length > 0,
	});

	// --- Step 3: Combine summary and details for the list component ---
	const certListData = useMemo<CertListItem[]>(() => {
		return certDomains.map((domain) => ({
			domain: domain,
			summary: certificates[domain],
			details: allDetailsMap?.[domain] ?? null,
		}));
	}, [certDomains, certificates, allDetailsMap]);

	// --- Calculate overview stats once all details are loaded ---
	const overviewStats = useMemo<CertOverviewStats>(() => {
		const certDetails = allDetailsMap ? Object.values(allDetailsMap) : [];
		if (certDomains.length === 0) {
			return {
				total: 0,
				valid: 0,
				expired: 0,
				soonestExpiryDays: null,
				uniqueFormats: 0,
				uniqueIssuers: 0,
				selfSigned: 0,
				uniqueAlgorithms: 0,
			};
		}
		const now = new Date();
		let soonestExpiryDays: number | null = null;
		const formats = new Set<string>();
		const issuers = new Set<string>();
		const algorithms = new Set<string>();
		let selfSigned = 0;

		for (const cert of certDetails) {
			const expiryDate = new Date(cert.validity.not_after);
			if (expiryDate > now) {
				const diffTime = expiryDate.getTime() - now.getTime();
				const diffDays = Math.ceil(diffTime / (1000 * 60 * 60 * 24));
				if (soonestExpiryDays === null || diffDays < soonestExpiryDays) {
					soonestExpiryDays = diffDays;
				}
			}
			issuers.add(
				cert.issuer.organization ||
					cert.issuer.common_name ||
					cert.issuer.raw_string
			);
			algorithms.add(cert.public_key.algorithm);
			if (cert.subject.raw_string === cert.issuer.raw_string) selfSigned++;
		}
		for (const summary of Object.values(certificates)) {
			formats.add(summary.format);
		}
		const validCount = certDetails.filter((c) => c.validity.is_valid).length;
		return {
			total: certDomains.length,
			valid: validCount,
			expired: certDomains.length - validCount,
			soonestExpiryDays,
			uniqueFormats: formats.size,
			uniqueIssuers: issuers.size,
			selfSigned,
			uniqueAlgorithms: algorithms.size,
		};
	}, [certificates, allDetailsMap, certDomains]);

	// --- Query for the details of the single SELECTED certificate ---
	const selectedCertDetailsQuery = useQuery<RequestResult<CertInfo>>({
		queryKey: ["instance", instanceId, "certs", "details", selectedDomain],
		queryFn: () => getCertDetails(instanceId, selectedDomain!),
		enabled: !!selectedDomain,
	});

	// --- UI State ---
	const isLoading =
		isListLoading || (certDomains.length > 0 && areDetailsLoading);
	const isError = isListError || isDetailsError;
	const error = listError || detailsError;

	// --- Handlers and Effects for selection (unchanged) ---
	const handleDomainSelect = useCallback(
		(domain: string | null) => {
			setSelectedDomain(domain);
			navigate({
				hash: domain ? encodeURIComponent(domain) : "",
				replace: true,
			});
		},
		[navigate]
	);

	useEffect(() => {
		if (isListLoading) return;
		const hashDomain = location.hash
			? decodeURIComponent(location.hash.slice(1))
			: null;
		if (hashDomain && certDomains.includes(hashDomain)) {
			if (selectedDomain !== hashDomain) setSelectedDomain(hashDomain);
			return;
		}
		if (!selectedDomain || !certDomains.includes(selectedDomain)) {
			handleDomainSelect(null);
		}
	}, [
		certDomains,
		isListLoading,
		location.hash,
		selectedDomain,
		handleDomainSelect,
	]);

	// --- Mutations for cert management ---
	const uploadMutation = useMutation<
		RequestResult<unknown>,
		Error,
		{ domain: string; certPem: string; keyPem: string }
	>({
		mutationFn: (vars) => {
			const payload = {
				cert_pem_b64: btoa(vars.certPem),
				key_pem_b64: btoa(vars.keyPem),
			};
			return uploadCert(instanceId, vars.domain, payload);
		},
		onSuccess: (_, vars) => {
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "certs"],
			});
			handleDomainSelect(vars.domain);
		},
	});

	const removeMutation = useMutation<RequestResult<unknown>, Error, string>({
		mutationFn: (domain) => deleteCert(instanceId, domain),
		onSuccess: () => {
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "certs"],
			});
		},
	});

	if (isLoading) {
		return <StatusCard icon={Server} text="Loading Certificate Details..." />;
	}
	if (isError) {
		return (
			<StatusCard
				icon={ServerCrash}
				text={error?.message || "Failed to fetch certificates."}
				isError
			/>
		);
	}

	return (
		<Tooltip.Provider delayDuration={200}>
			<div className="space-y-6">
				<CertOverviewCard stats={overviewStats} />
				<CertListCard
					certs={certListData} // Pass the combined data here
					selectedDomain={selectedDomain}
					onSelectDomain={handleDomainSelect}
					uploadMutation={uploadMutation}
					removeMutation={removeMutation}
				/>
				{selectedDomain && <CertDetailCard query={selectedCertDetailsQuery} />}
			</div>
		</Tooltip.Provider>
	);
}

// --- StatusCard Component (unchanged) ---
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
