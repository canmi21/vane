/* src/components/error-pages/template-editor-card.tsx */

import { useState, useEffect } from "react";
import { Code, Eye, Save, Loader2 } from "lucide-react";
import { useQuery, type UseMutationResult } from "@tanstack/react-query";
import { type RequestResult } from "~/api/request";
import { type TemplatePayload } from "~/routes/$instance/error-pages/$page";

export function TemplateEditorCard({
	instanceId,
	templateName,
	getTemplateContent,
	updateMutation,
}: {
	instanceId: string;
	templateName: string;
	getTemplateContent: (name: string) => Promise<RequestResult<TemplatePayload>>;
	updateMutation: UseMutationResult<
		RequestResult<unknown>,
		Error,
		{ name: string; content: string }
	>;
}) {
	const [htmlContent, setHtmlContent] = useState("");

	const { data, isLoading, isError, error } = useQuery({
		queryKey: ["instance", instanceId, "templates", templateName],
		queryFn: () => getTemplateContent(templateName),
	});

	useEffect(() => {
		if (data?.data?.html_base64) {
			try {
				const decodedContent = atob(data.data.html_base64);
				setHtmlContent(decodedContent);
			} catch (e) {
				console.error("Failed to decode base64 content", e);
				setHtmlContent("<!-- Error: Could not decode content -->");
			}
		}
	}, [data]);

	const handleSave = () => {
		updateMutation.mutate({ name: templateName, content: htmlContent });
	};

	const renderContent = () => {
		if (isLoading) {
			return (
				<div className="flex h-96 items-center justify-center gap-3">
					<Loader2
						size={24}
						className="animate-spin text-[var(--color-subtext)]"
					/>
					<p className="text-[var(--color-subtext)]">Loading template...</p>
				</div>
			);
		}
		if (isError) {
			return (
				<div className="p-6 text-center text-red-500">
					{error.message || "Failed to load template."}
				</div>
			);
		}
		return (
			<div className="grid grid-cols-1 gap-px bg-[var(--color-bg-alt)] lg:grid-cols-2">
				{/* Editor Pane */}
				<div className="flex flex-col bg-[var(--color-bg)]">
					<div className="flex items-center gap-2 border-b border-[var(--color-bg-alt)] p-3">
						<Code size={16} className="stroke-[var(--color-subtext)]" />
						<span className="text-sm font-medium text-[var(--color-text)]">
							Editor
						</span>
					</div>
					<textarea
						value={htmlContent}
						onChange={(e) => setHtmlContent(e.target.value)}
						placeholder="<!-- Your HTML code here -->"
						className="h-96 w-full flex-grow resize-none border-none bg-transparent p-3 font-mono text-xs text-[var(--color-text)] outline-none"
						spellCheck="false"
					/>
				</div>
				{/* Preview Pane */}
				<div className="flex flex-col bg-[var(--color-bg)]">
					<div className="flex items-center gap-2 border-b border-[var(--color-bg-alt)] p-3">
						<Eye size={16} className="stroke-[var(--color-subtext)]" />
						<span className="text-sm font-medium text-[var(--color-text)]">
							Preview
						</span>
					</div>
					<iframe
						srcDoc={htmlContent}
						title="Template Preview"
						className="h-96 w-full flex-grow border-none"
						sandbox="allow-scripts allow-same-origin"
					/>
				</div>
			</div>
		);
	};

	return (
		<div className="w-full overflow-hidden rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-sm">
			{/* Card Header */}
			<div className="flex items-center justify-between border-b border-[var(--color-bg-alt)] p-6">
				<h3 className="font-semibold text-[var(--color-text)]">
					Editing:{" "}
					<span className="font-mono text-[var(--color-theme-border)]">
						{templateName}.html
					</span>
				</h3>
				<div className="flex items-center gap-2">
					{updateMutation.isError && (
						<p className="text-xs text-red-500">
							{updateMutation.error.message || "Save failed."}
						</p>
					)}
					<button
						onClick={handleSave}
						disabled={updateMutation.isPending || isLoading}
						className="flex h-10 items-center gap-2 rounded-lg bg-[var(--color-theme-bg)] px-4 text-sm font-semibold text-[var(--color-text)] transition-all hover:opacity-80 disabled:cursor-not-allowed disabled:opacity-50"
					>
						<Save size={16} />
						{updateMutation.isPending ? "Saving..." : "Save Changes"}
					</button>
				</div>
			</div>
			{/* Editor and Preview */}
			{renderContent()}
		</div>
	);
}
