/* src/components/custom-header/header-editor-card.tsx */

import { useState, useEffect } from "react";
import {
	Save,
	Settings,
	RotateCcw,
	Trash2,
	PlusCircle,
	Loader2,
} from "lucide-react";
import {
	type UseQueryResult,
	type UseMutationResult,
} from "@tanstack/react-query";
import { type RequestResult } from "~/api/request";
import { type HeaderConfig } from "~/routes/$instance/custom-header/";

// --- Key-Value Input Component ---
function KeyValueInput({
	headers,
	onChange,
}: {
	headers: Record<string, string>;
	onChange: (newHeaders: Record<string, string>) => void;
}) {
	const headerEntries = Object.entries(headers);

	const handleKeyChange = (index: number, newKey: string) => {
		const newEntries = [...headerEntries];
		newEntries[index][0] = newKey;
		onChange(Object.fromEntries(newEntries));
	};

	const handleValueChange = (index: number, newValue: string) => {
		const newEntries = [...headerEntries];
		newEntries[index][1] = newValue;
		onChange(Object.fromEntries(newEntries));
	};

	const handleAddItem = () => {
		onChange({ ...headers, "": "" });
	};

	const handleRemoveItem = (keyToRemove: string) => {
		const newHeaders = { ...headers };
		delete newHeaders[keyToRemove];
		onChange(newHeaders);
	};

	return (
		<div className="space-y-3">
			{headerEntries.map(([key, value], index) => (
				<div key={index} className="flex items-center gap-2">
					<input
						type="text"
						value={key}
						onChange={(e) => handleKeyChange(index, e.target.value)}
						placeholder="Header-Name"
						className="input-field w-1/3 flex-grow"
					/>
					<input
						type="text"
						value={value}
						onChange={(e) => handleValueChange(index, e.target.value)}
						placeholder="Header-Value"
						className="input-field w-2/3 flex-grow"
					/>
					<button
						onClick={() => handleRemoveItem(key)}
						className="rounded-md p-2 text-[var(--color-subtext)] transition-colors hover:text-red-500"
					>
						<Trash2 size={16} />
					</button>
				</div>
			))}
			<button
				onClick={handleAddItem}
				className="flex items-center gap-2 rounded-md px-2 py-1 text-sm text-[var(--color-theme-border)] transition-colors hover:bg-[var(--color-theme-bg)]"
			>
				<PlusCircle size={16} /> Add Header
			</button>
		</div>
	);
}

// --- Main Editor Card Component ---
export function HeaderEditorCard({
	domain,
	query,
	updateMutation,
	resetMutation,
}: {
	domain: string;
	query: UseQueryResult<RequestResult<HeaderConfig>>;
	updateMutation: UseMutationResult<
		RequestResult<HeaderConfig>,
		Error,
		{ domain: string; config: HeaderConfig }
	>;
	resetMutation: UseMutationResult<RequestResult<unknown>, Error, string>;
}) {
	const { data, isLoading, isError, error } = query;
	const [config, setConfig] = useState<HeaderConfig | null>(null);

	useEffect(() => {
		if (data?.data) {
			setConfig(JSON.parse(JSON.stringify(data.data)));
		}
	}, [data]);

	const handleSave = () => {
		if (config) {
			// Filter out any entries where key or value is empty
			const cleanedHeaders = Object.fromEntries(
				Object.entries(config.headers).filter(
					([key, value]) => key.trim() !== "" && value.trim() !== ""
				)
			);
			updateMutation.mutate({ domain, config: { headers: cleanedHeaders } });
		}
	};

	const handleReset = () => {
		if (window.confirm(`Reset custom headers for "${domain}" to default?`)) {
			resetMutation.mutate(domain);
		}
	};

	if (isLoading)
		return (
			<div className="flex h-64 items-center justify-center">
				<Loader2
					size={24}
					className="animate-spin text-[var(--color-subtext)]"
				/>
			</div>
		);
	if (isError)
		return <div className="p-6 text-center text-red-500">{error.message}</div>;
	if (!config) return null;

	return (
		<div className="w-full rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-sm">
			<div className="flex items-center justify-between border-b border-[var(--color-bg-alt)] p-6">
				<div className="flex items-center gap-3">
					<Settings size={20} className="stroke-[var(--color-theme-border)]" />
					<h3 className="font-semibold text-[var(--color-text)]">
						Header Policy for{" "}
						<span className="font-mono text-[var(--color-theme-border)]">
							{domain}
						</span>
					</h3>
				</div>
				<div className="flex items-center gap-2">
					{updateMutation.isError && (
						<p className="text-xs text-red-500">
							{updateMutation.error.message}
						</p>
					)}
					<button
						onClick={handleReset}
						disabled={resetMutation.isPending || updateMutation.isPending}
						className="flex h-10 items-center gap-2 rounded-lg px-3 text-sm font-semibold text-[var(--color-subtext)] transition-all hover:bg-[var(--color-bg-alt)] hover:text-[var(--color-text)] disabled:opacity-50"
					>
						<RotateCcw size={16} /> Reset
					</button>
					<button
						onClick={handleSave}
						disabled={updateMutation.isPending || resetMutation.isPending}
						className="flex h-10 items-center gap-2 rounded-lg bg-[var(--color-theme-bg)] px-4 text-sm font-semibold text-[var(--color-text)] transition-all hover:opacity-80 disabled:opacity-50"
					>
						<Save size={16} />{" "}
						{updateMutation.isPending ? "Saving..." : "Save Changes"}
					</button>
				</div>
			</div>
			<div className="p-6">
				<KeyValueInput
					headers={config.headers}
					onChange={(newHeaders) =>
						setConfig({ ...config, headers: newHeaders })
					}
				/>
			</div>
		</div>
	);
}
