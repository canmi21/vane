/* src/components/cors/cors-editor-card.tsx */

import React, { useState, useEffect } from "react";
import { Save, Loader2, Settings, RotateCcw } from "lucide-react";
import * as RadixSwitch from "@radix-ui/react-switch";
import * as RadixRadioGroup from "@radix-ui/react-radio-group";
import {
	type UseQueryResult,
	type UseMutationResult,
} from "@tanstack/react-query";
import { type RequestResult } from "~/api/request";
import {
	type CorsConfig,
	type PreflightHandling,
} from "~/routes/$instance/cors-management/";

// --- Helper for array-to-string conversion ---
const arrayToString = (arr: string[]) => arr.join(", ");
const stringToArray = (str: string) =>
	str
		? str
				.split(",")
				.map((s) => s.trim())
				.filter(Boolean)
		: [];

// --- Form Input Component ---
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
	const [config, setConfig] = useState<CorsConfig | null>(null);
	const { data, isLoading, isError, error } = query; // Use the passed query object

	useEffect(() => {
		if (data?.data) {
			// Deep copy to prevent mutating the react-query cache directly
			setConfig(JSON.parse(JSON.stringify(data.data)));
		}
	}, [data]);

	const handleSave = () => {
		if (config) {
			updateMutation.mutate({ domain, config });
		}
	};

	const handleReset = () => {
		if (window.confirm(`Reset CORS config for "${domain}" to defaults?`)) {
			resetMutation.mutate(domain);
		}
	};

	if (isLoading) {
		return (
			<div className="flex items-center justify-center p-12">
				<Loader2 className="animate-spin" />
			</div>
		);
	}
	if (isError) {
		return <div className="p-6 text-center text-red-500">{error.message}</div>;
	}
	if (!config) {
		return null; // or a placeholder
	}

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
						disabled={updateMutation.isPending || resetMutation.isPending}
						className="flex h-10 items-center gap-2 rounded-lg bg-[var(--color-theme-bg)] px-4 text-sm font-semibold text-[var(--color-text)] transition-all hover:opacity-80 disabled:opacity-50"
					>
						<Save size={16} />{" "}
						{updateMutation.isPending ? "Saving..." : "Save Changes"}
					</button>
				</div>
			</div>

			<div className="p-6">
				<FormRow
					label="Preflight Handling"
					description="Choose who responds to OPTIONS requests."
				>
					<RadixRadioGroup.Root
						value={config.preflight_handling}
						onValueChange={(v) =>
							setConfig({
								...config,
								preflight_handling: v as PreflightHandling,
							})
						}
						className="flex gap-6"
					>
						<div className="flex items-center gap-2">
							<RadixRadioGroup.Item
								value="proxy_decision"
								id="r1"
								className="h-4 w-4 rounded-full border border-[var(--color-tertiary)] focus:outline-none focus:ring-1 focus:ring-[var(--color-theme-border)]"
							>
								<RadixRadioGroup.Indicator className="relative flex h-full w-full items-center justify-center after:block after:h-2 after:w-2 after:rounded-full after:bg-[var(--color-theme-border)]" />
							</RadixRadioGroup.Item>
							<label htmlFor="r1" className="text-sm">
								Vane Proxy
							</label>
						</div>
						<div className="flex items-center gap-2">
							<RadixRadioGroup.Item
								value="origin_response"
								id="r2"
								className="h-4 w-4 rounded-full border border-[var(--color-tertiary)] focus:outline-none focus:ring-1 focus:ring-[var(--color-theme-border)]"
							>
								<RadixRadioGroup.Indicator className="relative flex h-full w-full items-center justify-center after:block after:h-2 after:w-2 after:rounded-full after:bg-[var(--color-theme-border)]" />
							</RadixRadioGroup.Item>
							<label htmlFor="r2" className="text-sm">
								Origin Server
							</label>
						</div>
					</RadixRadioGroup.Root>
				</FormRow>

				<FormRow
					label="Allowed Origins"
					description="Comma-separated list of origins. Use '*' for any origin (not secure with credentials)."
				>
					<input
						type="text"
						value={arrayToString(config.allow_origins)}
						onChange={(e) =>
							setConfig({
								...config,
								allow_origins: stringToArray(e.target.value),
							})
						}
						className="input-field"
					/>
				</FormRow>

				<FormRow
					label="Allowed Methods"
					description="Comma-separated list of HTTP methods (e.g., GET, POST)."
				>
					<input
						type="text"
						value={arrayToString(config.allow_methods)}
						onChange={(e) =>
							setConfig({
								...config,
								allow_methods: stringToArray(e.target.value),
							})
						}
						className="input-field"
					/>
				</FormRow>

				<FormRow
					label="Allowed Headers"
					description="Comma-separated list of custom headers clients can send."
				>
					<input
						type="text"
						value={arrayToString(config.allow_headers)}
						onChange={(e) =>
							setConfig({
								...config,
								allow_headers: stringToArray(e.target.value),
							})
						}
						className="input-field"
					/>
				</FormRow>

				<FormRow
					label="Exposed Headers"
					description="Comma-separated list of headers clients can access in responses."
				>
					<input
						type="text"
						value={arrayToString(config.expose_headers)}
						onChange={(e) =>
							setConfig({
								...config,
								expose_headers: stringToArray(e.target.value),
							})
						}
						className="input-field"
					/>
				</FormRow>

				<FormRow
					label="Max Age (seconds)"
					description="How long preflight results can be cached by the browser."
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
					description="Allows cookies and other credentials to be included in cross-origin requests."
				>
					<RadixSwitch.Root
						checked={config.allow_credentials}
						onCheckedChange={(c) =>
							setConfig({ ...config, allow_credentials: c })
						}
						className="relative h-6 w-11 rounded-full bg-[var(--color-bg-alt)] data-[state=checked]:bg-[var(--color-theme-border)]"
					>
						<RadixSwitch.Thumb className="block h-5 w-5 translate-x-0.5 rounded-full bg-white transition-transform duration-100 will-change-transform data-[state=checked]:translate-x-[1.1rem]" />
					</RadixSwitch.Root>
				</FormRow>
			</div>
		</div>
	);
}
