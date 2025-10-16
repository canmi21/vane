/* src/routes/$instance/error-pages/index.tsx */

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
import {
	getInstance,
	postInstance,
	putInstance,
	deleteInstance,
} from "~/api/instance";
import { TemplateListCard } from "~/components/error-pages/template-list-card";
import { TemplateEditorCard } from "~/components/error-pages/template-editor-card";
import * as Tooltip from "@radix-ui/react-tooltip";

// --- API Helper Functions ---
async function listTemplates(
	instanceId: string
): Promise<RequestResult<ListTemplatesResponse>> {
	return getInstance(instanceId, "/v1/templates");
}

async function getTemplateContent(
	instanceId: string,
	name: string
): Promise<RequestResult<TemplatePayload>> {
	return getInstance(instanceId, `/v1/templates/${name}`);
}

async function createTemplate(
	instanceId: string,
	name: string,
	htmlContent: string
): Promise<RequestResult<unknown>> {
	const payload = { html_base64: btoa(htmlContent) };
	return postInstance(instanceId, `/v1/templates/${name}`, payload);
}

async function updateTemplate(
	instanceId: string,
	name: string,
	htmlContent: string
): Promise<RequestResult<unknown>> {
	const payload = { html_base64: btoa(htmlContent) };
	return putInstance(instanceId, `/v1/templates/${name}`, payload);
}

async function deleteTemplate(
	instanceId: string,
	name: string
): Promise<RequestResult<unknown>> {
	return deleteInstance(instanceId, `/v1/templates/${name}`);
}

// --- Data Types from Backend ---
export interface ListTemplatesResponse {
	templates: string[];
}

export interface TemplatePayload {
	html_base64: string;
}

export const Route = createFileRoute("/$instance/error-pages/")({
	component: ErrorPages,
});

function ErrorPages() {
	const { instance: instanceId } = useParams({
		from: "/$instance/error-pages/",
	});
	const queryClient = useQueryClient();
	const navigate = useNavigate();
	const location = useLocation();

	const [selectedTemplate, setSelectedTemplate] = useState<string | null>(null);

	// --- Query for the list of all templates ---
	const {
		data: templatesResult,
		isLoading,
		isError,
		error,
	} = useQuery<RequestResult<ListTemplatesResponse>>({
		queryKey: ["instance", instanceId, "templates"],
		queryFn: () => listTemplates(instanceId),
	});

	const templates = useMemo(
		() =>
			templatesResult?.data?.templates.map((t) => t.replace(".html", "")) ?? [],
		[templatesResult]
	);

	// --- Handler to manage selection and URL hash sync ---
	const handleTemplateSelect = useCallback(
		(name: string | null) => {
			setSelectedTemplate(name);
			navigate({ hash: name ? encodeURIComponent(name) : "", replace: true });
		},
		[navigate]
	);

	// --- Logic to sync state from URL on load and data changes ---
	useEffect(() => {
		if (isLoading) return;
		const hashTemplate = location.hash
			? decodeURIComponent(location.hash.slice(1))
			: null;
		if (hashTemplate && templates.includes(hashTemplate)) {
			if (selectedTemplate !== hashTemplate) {
				setSelectedTemplate(hashTemplate);
			}
			return;
		}
		if (!selectedTemplate || !templates.includes(selectedTemplate)) {
			handleTemplateSelect(null);
		}
	}, [
		templates,
		isLoading,
		location.hash,
		selectedTemplate,
		handleTemplateSelect,
	]);

	// --- Mutations for template management ---
	const createMutation = useMutation<
		RequestResult<unknown>,
		Error,
		{ name: string; content: string }
	>({
		mutationFn: (vars) => createTemplate(instanceId, vars.name, vars.content),
		onSuccess: (_, vars) => {
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "templates"],
			});
			handleTemplateSelect(vars.name); // Select the new template for editing
		},
	});

	const updateMutation = useMutation<
		RequestResult<unknown>,
		Error,
		{ name: string; content: string }
	>({
		mutationFn: (vars) => updateTemplate(instanceId, vars.name, vars.content),
		onSuccess: () => {
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "templates", selectedTemplate],
			});
		},
	});

	const removeMutation = useMutation<RequestResult<unknown>, Error, string>({
		mutationFn: (name) => deleteTemplate(instanceId, name),
		onSuccess: () => {
			queryClient.invalidateQueries({
				queryKey: ["instance", instanceId, "templates"],
			});
		},
	});

	if (isLoading) {
		return <StatusCard icon={Server} text="Loading Error Pages..." />;
	}
	if (isError) {
		return (
			<StatusCard
				icon={ServerCrash}
				text={error?.message || "Failed to fetch error pages."}
				isError
			/>
		);
	}

	return (
		<Tooltip.Provider delayDuration={200}>
			<div className="space-y-6">
				<TemplateListCard
					templates={templates}
					selectedTemplate={selectedTemplate}
					onSelectTemplate={handleTemplateSelect}
					createMutation={createMutation}
					removeMutation={removeMutation}
				/>
				{selectedTemplate && (
					<TemplateEditorCard
						key={selectedTemplate} // Force re-mount on selection change
						instanceId={instanceId}
						templateName={selectedTemplate}
						getTemplateContent={(name) => getTemplateContent(instanceId, name)}
						updateMutation={updateMutation}
					/>
				)}
			</div>
		</Tooltip.Provider>
	);
}

// --- StatusCard Component ---
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
