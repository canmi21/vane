/* src/components/cors/cors-editor-card.tsx */

import React, { useState, useEffect, useMemo } from "react";
import { motion } from "framer-motion";
import {
	Save,
	Settings,
	RotateCcw,
	Trash2,
	PlusCircle,
	AlertCircle,
	Loader2,
} from "lucide-react";
import {
	type UseQueryResult,
	type UseMutationResult,
} from "@tanstack/react-query";
import { type RequestResult } from "~/api/request";
import {
	type CorsConfig,
	type PreflightHandling,
} from "~/routes/$instance/cors-management/$domain";

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

// --- Multi-value input component ---
function MultiValueInput({
	values,
	onChange,
	placeholder,
}: {
	values: string[];
	onChange: (newValues: string[]) => void;
	placeholder: string;
}) {
	const handleItemChange = (index: number, value: string) => {
		onChange(values.map((v, i) => (i === index ? value : v)));
	};
	const handleAddItem = () => {
		onChange([...values, ""]);
	};
	const handleRemoveItem = (index: number) => {
		onChange(values.filter((_, i) => i !== index));
	};
	return (
		<div className="space-y-2">
			{values.map((value, index) => (
				<div key={index} className="flex items-center gap-2">
					<input
						type="text"
						value={value}
						onChange={(e) => handleItemChange(index, e.target.value)}
						placeholder={placeholder}
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
				<PlusCircle size={16} /> Add
			</button>
		</div>
	);
}

// --- Form Input Row Component ---
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

// --- Main Editor Card Component ---
export function CorsEditorCard({
	domain,
	query,
	updateMutation,
	resetMutation,
}: {
	domain: string;
	query: UseQueryResult<RequestResult<CorsConfig>>;
	updateMutation: UseMutationResult<
		RequestResult<CorsConfig>,
		Error,
		{ domain: string; config: CorsConfig }
	>;
	resetMutation: UseMutationResult<RequestResult<unknown>, Error, string>;
}) {
	const { data, isLoading, isError, error } = query;
	const [config, setConfig] = useState<CorsConfig | null>(null);

	const validationError = useMemo<string | null>(() => {
		if (config?.preflight_handling === "proxy_decision") {
			const hasOptions =
				config.allow_methods.includes("*") ||
				config.allow_methods.some((m) => m.toUpperCase() === "OPTIONS");
			if (!hasOptions) {
				return "When Vane Proxy handles preflights, 'OPTIONS' or '*' must be in Allowed Methods.";
			}
		}
		return null;
	}, [config]);

	useEffect(() => {
		if (data?.data) {
			setConfig(JSON.parse(JSON.stringify(data.data)));
		}
	}, [data]);

	const handleSave = () => {
		if (config && !validationError) {
			updateMutation.mutate({ domain, config });
		}
	};

	const handleReset = () => {
		if (window.confirm(`Reset CORS config for "${domain}" to defaults?`)) {
			resetMutation.mutate(domain);
		}
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
						CORS Policy for{" "}
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
						disabled={
							updateMutation.isPending ||
							resetMutation.isPending ||
							!!validationError
						}
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
						label="Preflight Handling"
						description="Choose who responds to OPTIONS requests."
					>
						<SmallToggleSlider
							value={config.preflight_handling}
							onValueChange={(v: PreflightHandling) =>
								setConfig({ ...config, preflight_handling: v })
							}
							optionTrue="proxy_decision"
							labelTrue="Vane Proxy"
							optionFalse="origin_response"
							labelFalse="Origin Server"
						/>
					</FormRow>
					<FormRow
						label="Allowed Origins"
						description="List of origins. Use '*' for any origin (not secure with credentials)."
					>
						<MultiValueInput
							values={config.allow_origins}
							onChange={(v) => setConfig({ ...config, allow_origins: v })}
							placeholder="https://example.com"
						/>
					</FormRow>
					<FormRow
						label="Allowed Methods"
						description="List of HTTP methods (e.g., GET, POST)."
					>
						<>
							<MultiValueInput
								values={config.allow_methods}
								onChange={(v) => setConfig({ ...config, allow_methods: v })}
								placeholder="GET"
							/>
							{validationError && (
								<div className="mt-2 flex items-center gap-2 text-xs text-red-500">
									<AlertCircle size={14} />
									<span>{validationError}</span>
								</div>
							)}
						</>
					</FormRow>
					<FormRow
						label="Allowed Headers"
						description="List of custom headers clients can send."
					>
						<MultiValueInput
							values={config.allow_headers}
							onChange={(v) => setConfig({ ...config, allow_headers: v })}
							placeholder="Content-Type"
						/>
					</FormRow>
					<FormRow
						label="Exposed Headers"
						description="List of headers clients can access in responses."
					>
						<MultiValueInput
							values={config.expose_headers}
							onChange={(v) => setConfig({ ...config, expose_headers: v })}
							placeholder="X-My-Custom-Header"
						/>
					</FormRow>
					<FormRow
						label="Max Age (seconds)"
						description="How long preflight results can be cached."
					>
						<input
							type="number"
							value={config.max_age_seconds}
							onChange={(e) =>
								setConfig({
									...config,
									max_age_seconds: Number(e.target.value) || 0,
								})
							}
							className="input-field w-32"
						/>
					</FormRow>
					<FormRow
						label="Allow Credentials"
						description="Allows cookies and credentials in requests."
					>
						<SmallToggleSlider
							value={config.allow_credentials}
							onValueChange={(v: boolean) =>
								setConfig({ ...config, allow_credentials: v })
							}
							optionTrue={true}
							labelTrue="True"
							optionFalse={false}
							labelFalse="False"
						/>
					</FormRow>
				</div>
			</div>
		</div>
	);
}
