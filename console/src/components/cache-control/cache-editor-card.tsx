/* src/components/cache-control/cache-editor-card.tsx */

import React, { useState, useEffect } from "react";
import { motion } from "framer-motion";
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
import {
	type CacheConfig,
	type CacheRule,
} from "~/routes/$instance/cache-control/$domain";

// --- Reusable Small Toggle Slider Component ---
function SmallToggleSlider<T>({
	value,
	onValueChange,
	optionTrue,
	optionFalse,
	labelTrue,
	labelFalse,
}: {
	value: T;
	onValueChange: (newValue: T) => void;
	optionTrue: T;
	optionFalse: T;
	labelTrue: string;
	labelFalse: string;
}) {
	const isOn = value === optionTrue;
	return (
		<div
			className="relative flex w-48 cursor-pointer items-center rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg-alt)] p-1"
			onClick={() => onValueChange(isOn ? optionFalse : optionTrue)}
		>
			<motion.div
				className="absolute h-[calc(100%-8px)] w-[calc(50%-4px)] rounded-md bg-[var(--color-bg)] shadow-sm"
				style={{ top: "4px", left: "4px" }}
				animate={{ x: isOn ? 0 : "100%" }}
				transition={{ type: "spring", stiffness: 300, damping: 30 }}
			/>
			<div
				className={`relative z-10 flex-1 py-1 text-center text-xs font-semibold transition-colors ${isOn ? "text-[var(--color-text)]" : "text-[var(--color-subtext)]"}`}
			>
				{labelTrue}
			</div>
			<div
				className={`relative z-10 flex-1 py-1 text-center text-xs font-semibold transition-colors ${!isOn ? "text-[var(--color-text)]" : "text-[var(--color-subtext)]"}`}
			>
				{labelFalse}
			</div>
		</div>
	);
}

// --- Reusable Form Input Row Component ---
function FormRow({
	label,
	description,
	children,
}: {
	label: string;
	description: string;
	children: React.ReactNode;
}) {
	return (
		<div className="grid grid-cols-1 gap-2 border-b border-[var(--color-bg-alt)] py-4 sm:grid-cols-3 sm:gap-4">
			<div className="sm:col-span-1">
				<label className="text-sm font-medium text-[var(--color-text)]">
					{label}
				</label>
				<p className="mt-1 text-xs text-[var(--color-subtext)]">
					{description}
				</p>
			</div>
			<div className="sm:col-span-2">{children}</div>
		</div>
	);
}

// --- Input component for path rules ---
function PathRuleInput({
	rules,
	onChange,
}: {
	rules: CacheRule[];
	onChange: (newRules: CacheRule[]) => void;
}) {
	const handleItemChange = (
		index: number,
		field: keyof CacheRule,
		value: string | number
	) => {
		const newRules = [...rules];
		newRules[index] = { ...newRules[index], [field]: value };
		onChange(newRules);
	};
	const handleAddItem = () =>
		onChange([...rules, { path: "", ttl_seconds: 3600 }]);
	const handleRemoveItem = (index: number) =>
		onChange(rules.filter((_, i) => i !== index));

	return (
		<div className="space-y-2">
			{rules.map((rule, index) => (
				<div key={index} className="flex items-center gap-2">
					<input
						type="text"
						value={rule.path}
						onChange={(e) => handleItemChange(index, "path", e.target.value)}
						placeholder="/images/*"
						className="input-field flex-grow"
					/>
					<input
						type="number"
						value={rule.ttl_seconds}
						onChange={(e) =>
							handleItemChange(
								index,
								"ttl_seconds",
								Number(e.target.value) || 0
							)
						}
						className="input-field w-32"
					/>
					<span className="text-xs text-[var(--color-subtext)]">seconds</span>
					<button
						onClick={() => handleRemoveItem(index)}
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
				<PlusCircle size={16} /> Add Rule
			</button>
		</div>
	);
}

// --- Input component for blacklist paths ---
function BlacklistPathInput({
	paths,
	onChange,
}: {
	paths: string[];
	onChange: (newPaths: string[]) => void;
}) {
	const handleItemChange = (index: number, value: string) => {
		onChange(paths.map((p, i) => (i === index ? value : p)));
	};
	const handleAddItem = () => onChange([...paths, ""]);
	const handleRemoveItem = (index: number) =>
		onChange(paths.filter((_, i) => i !== index));

	return (
		<div className="space-y-2">
			{paths.map((path, index) => (
				<div key={index} className="flex items-center gap-2">
					<input
						type="text"
						value={path}
						onChange={(e) => handleItemChange(index, e.target.value)}
						placeholder="/api/*"
						className="input-field flex-grow"
					/>
					<button
						onClick={() => handleRemoveItem(index)}
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
				<PlusCircle size={16} /> Add Path
			</button>
		</div>
	);
}

// --- Main Editor Card Component ---
export function CacheEditorCard({
	domain,
	query,
	updateMutation,
	resetMutation,
}: {
	domain: string;
	query: UseQueryResult<RequestResult<CacheConfig>>;
	updateMutation: UseMutationResult<
		RequestResult<CacheConfig>,
		Error,
		{ domain: string; config: CacheConfig }
	>;
	resetMutation: UseMutationResult<RequestResult<unknown>, Error, string>;
}) {
	const { data, isLoading, isError, error } = query;
	const [config, setConfig] = useState<CacheConfig | null>(null);

	useEffect(() => {
		if (data?.data) setConfig(JSON.parse(JSON.stringify(data.data)));
	}, [data]);

	const handleSave = () => {
		if (config) {
			const cleanedConfig = {
				...config,
				path_rules: config.path_rules.filter((r) => r.path.trim() !== ""),
				blacklist_paths: config.blacklist_paths.filter((p) => p.trim() !== ""),
			};
			updateMutation.mutate({ domain, config: cleanedConfig });
		}
	};

	const handleReset = () => {
		if (window.confirm(`Reset cache config for "${domain}" to default?`))
			resetMutation.mutate(domain);
	};

	if (isLoading)
		return (
			<div className="flex h-96 items-center justify-center">
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
						Cache Policy for{" "}
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
				{/* --- FIX: Wrapped FormRow list in a div with negative margins (-my-4) --- */}
				{/* This counteracts the py-4 on FormRow, fixing the extra padding issue. */}
				<div className="-my-4">
					<FormRow
						label="Origin Headers"
						description="If enabled, Vane will honor 'Cache-Control' headers from your origin server."
					>
						<SmallToggleSlider
							value={config.respect_origin_cache_control}
							onValueChange={(v: boolean) =>
								setConfig({ ...config, respect_origin_cache_control: v })
							}
							optionTrue={true}
							labelTrue="Respect"
							optionFalse={false}
							labelFalse="Ignore"
						/>
					</FormRow>

					<FormRow
						label="Path-Specific Rules"
						description="Define custom cache TTLs for specific URL paths. Paths can include wildcards (*)."
					>
						<PathRuleInput
							rules={config.path_rules}
							onChange={(newRules) =>
								setConfig({ ...config, path_rules: newRules })
							}
						/>
					</FormRow>

					<FormRow
						label="Do Not Cache Paths"
						description="A list of paths that should never be cached, regardless of other rules."
					>
						<BlacklistPathInput
							paths={config.blacklist_paths}
							onChange={(newPaths) =>
								setConfig({ ...config, blacklist_paths: newPaths })
							}
						/>
					</FormRow>
				</div>
			</div>
		</div>
	);
}
