/* src/components/rate-limit/rate-limit-editor-card.tsx */

import { useState, useEffect } from "react";
import { Save, Settings, RotateCcw, Loader2 } from "lucide-react";
import * as RadixSlider from "@radix-ui/react-slider";
import {
	type UseQueryResult,
	type UseMutationResult,
} from "@tanstack/react-query";
import { type RequestResult } from "~/api/request";
import { type RateLimitConfig } from "~/routes/$instance/rate-limit/$domain";

// --- Main Editor Card Component ---
export function RateLimitEditorCard({
	domain,
	query,
	updateMutation,
	resetMutation,
}: {
	domain: string;
	query: UseQueryResult<RequestResult<RateLimitConfig>>;
	updateMutation: UseMutationResult<
		RequestResult<RateLimitConfig>,
		Error,
		{ domain: string; config: RateLimitConfig }
	>;
	resetMutation: UseMutationResult<RequestResult<unknown>, Error, string>;
}) {
	const { data, isLoading, isError, error } = query;
	const [config, setConfig] = useState<RateLimitConfig | null>(null);

	useEffect(() => {
		if (data?.data) {
			setConfig(JSON.parse(JSON.stringify(data.data)));
		}
	}, [data]);

	const handleSave = () => {
		if (config) {
			updateMutation.mutate({ domain, config });
		}
	};

	const handleReset = () => {
		if (
			window.confirm(`Reset rate limit for "${domain}" to default (disabled)?`)
		) {
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

	const rps = config.requests_per_second;

	return (
		<div className="w-full rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-sm">
			<div className="flex items-center justify-between border-b border-[var(--color-bg-alt)] p-6">
				<div className="flex items-center gap-3">
					<Settings size={20} className="stroke-[var(--color-theme-border)]" />
					<h3 className="font-semibold text-[var(--color-text)]">
						Rate Limit for{" "}
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
				<div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
					<div className="sm:col-span-1">
						<label className="text-sm font-medium text-[var(--color-text)]">
							Requests Per Second
						</label>
						<p className="mt-1 text-xs text-[var(--color-subtext)]">
							Set the maximum number of requests per second. Use 0 to disable
							the limit.
						</p>
					</div>
					<div className="sm:col-span-2">
						<div className="flex items-center gap-4">
							{/* --- MODIFIED: Changed max to 1000 --- */}
							<RadixSlider.Root
								value={[rps]}
								onValueChange={(v) => setConfig({ requests_per_second: v[0] })}
								max={1000}
								step={1}
								className="relative flex h-5 w-full touch-none select-none items-center"
							>
								<RadixSlider.Track className="relative h-1 flex-grow rounded-full bg-[var(--color-tertiary)]">
									<RadixSlider.Range className="absolute h-full rounded-full bg-[var(--color-theme-border)]" />
								</RadixSlider.Track>
								<RadixSlider.Thumb className="block h-4 w-4 rounded-full bg-white shadow-md focus:outline-none focus:ring-2 focus:ring-[var(--color-theme-border)]" />
							</RadixSlider.Root>
							<div className="flex w-36 items-center gap-2">
								<input
									type="number"
									value={rps}
									onChange={(e) =>
										setConfig({
											requests_per_second: Math.max(
												0,
												Number(e.target.value) || 0
											),
										})
									}
									className="input-field w-20 text-center"
								/>
								<span className="text-xs text-[var(--color-subtext)]">
									req/s
								</span>
							</div>
						</div>
						<div className="mt-2 text-center text-sm font-medium text-[var(--color-text)]">
							{rps === 0
								? "Rate Limiting is Disabled"
								: `Limit: ${rps} requests per second`}
						</div>
					</div>
				</div>
			</div>
		</div>
	);
}
