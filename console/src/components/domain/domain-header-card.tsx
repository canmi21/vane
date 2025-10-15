/* src/components/domain/domain-header-card.tsx */

import { useState } from "react";
import * as Select from "@radix-ui/react-select";
import { motion, AnimatePresence } from "framer-motion";
import {
	Network,
	ChevronDown,
	Check,
	Plus,
	X,
	Save,
	Trash2,
} from "lucide-react";
import { type UseMutationResult } from "@tanstack/react-query";
import { type RequestResult } from "~/api/request";

export function DomainHeaderCard({
	domains,
	selectedDomain,
	setSelectedDomain,
	addMutation,
	removeMutation,
}: {
	domains: string[];
	selectedDomain: string | null;
	setSelectedDomain: (domain: string) => void;
	addMutation: UseMutationResult<RequestResult<unknown>, Error, string>;
	removeMutation: UseMutationResult<RequestResult<unknown>, Error, string>;
}) {
	const [isAdding, setIsAdding] = useState(false);
	const [newDomain, setNewDomain] = useState("");

	const handleAddDomain = (e: React.FormEvent) => {
		e.preventDefault();
		if (newDomain.trim()) {
			addMutation.mutate(newDomain.trim(), {
				onSuccess: () => {
					setNewDomain("");
					setIsAdding(false);
				},
			});
		}
	};

	const handleRemoveSelectedDomain = () => {
		if (
			selectedDomain &&
			window.confirm(
				`Are you sure you want to delete the domain "${selectedDomain}"?`
			)
		) {
			removeMutation.mutate(selectedDomain);
		}
	};

	return (
		<div className="w-full rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-sm">
			{/* Card Header Section for all controls */}
			<div className="border-b border-[var(--color-bg-alt)] p-6">
				<div className="flex items-center justify-between">
					{/* Left side: Title */}
					<div className="flex items-center gap-3">
						<Network size={20} className="stroke-[var(--color-theme-border)]" />
						<h3 className="text-lg font-semibold text-[var(--color-text)]">
							Domain Configuration
						</h3>
					</div>

					{/* Right side: Controls */}
					<div className="flex items-center gap-2">
						<Select.Root
							value={selectedDomain ?? ""}
							onValueChange={setSelectedDomain}
							disabled={isAdding}
						>
							<Select.Trigger className="flex h-10 w-64 items-center justify-between rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg-alt)] px-3 text-sm text-[var(--color-text)] transition-all hover:border-[var(--color-theme-border)] focus:border-[var(--color-theme-border)] focus:outline-none focus:ring-1 focus:ring-[var(--color-theme-border)] disabled:cursor-not-allowed disabled:opacity-50">
								<Select.Value placeholder="Select a domain..." />
								<Select.Icon>
									<ChevronDown size={16} />
								</Select.Icon>
							</Select.Trigger>
							<Select.Portal>
								<Select.Content
									position="popper"
									sideOffset={5}
									className="z-50 w-[--radix-select-trigger-width] overflow-hidden rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-lg"
								>
									<Select.Viewport className="p-1">
										{domains.map((domain) => (
											<Select.Item
												key={domain}
												value={domain}
												// --- STYLING FIX: Added my-0.5 for vertical margin ---
												className="relative flex cursor-pointer select-none items-center justify-between rounded-md my-0.5 py-1.5 pl-8 pr-2 text-sm text-[var(--color-text)] outline-none hover:bg-[var(--color-theme-bg)] data-[state=checked]:bg-[var(--color-theme-bg)]"
											>
												<Select.ItemText>{domain}</Select.ItemText>
												<Select.ItemIndicator className="absolute left-2">
													<Check size={16} />
												</Select.ItemIndicator>
											</Select.Item>
										))}
									</Select.Viewport>
								</Select.Content>
							</Select.Portal>
						</Select.Root>

						<button
							onClick={handleRemoveSelectedDomain}
							disabled={!selectedDomain || removeMutation.isPending || isAdding}
							className="flex h-10 w-10 items-center justify-center rounded-lg text-[var(--color-subtext)] transition-all hover:bg-[var(--color-bg-alt)] hover:text-red-500 disabled:cursor-not-allowed disabled:opacity-50"
							title={
								selectedDomain
									? `Delete ${selectedDomain}`
									: "No domain selected"
							}
						>
							<Trash2 size={16} />
						</button>

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
									<Plus size={16} /> Add
								</>
							)}
						</button>
					</div>
				</div>
			</div>

			{/* Collapsible Add Domain Form */}
			<AnimatePresence>
				{isAdding && (
					<motion.div
						initial={{ height: 0, opacity: 0 }}
						animate={{ height: "auto", opacity: 1 }}
						exit={{ height: 0, opacity: 0 }}
						transition={{ duration: 0.3, ease: "easeInOut" }}
						className="overflow-hidden border-b border-[var(--color-bg-alt)]"
					>
						<div className="p-4">
							<form onSubmit={handleAddDomain} className="flex gap-2">
								<input
									type="text"
									value={newDomain}
									onChange={(e) => setNewDomain(e.target.value)}
									placeholder="example.com or *.example.com"
									className="h-10 flex-grow rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg-alt)] px-3 text-sm text-[var(--color-text)] placeholder-[var(--color-subtext)] transition-all focus:border-[var(--color-theme-border)] focus:outline-none focus:ring-1 focus:ring-[var(--color-theme-border)]"
									disabled={addMutation.isPending}
									autoFocus
								/>
								<button
									type="submit"
									className="flex h-10 items-center gap-2 rounded-lg bg-[var(--color-theme-bg)] px-4 text-sm font-semibold text-[var(--color-text)] transition-all hover:opacity-80 disabled:cursor-not-allowed disabled:opacity-50"
									disabled={addMutation.isPending || !newDomain.trim()}
								>
									<Save size={16} /> Add Domain
								</button>
							</form>
							{addMutation.isError && (
								<p className="mt-2 text-xs text-red-500">
									{addMutation.error?.message || "Failed to add domain."}
								</p>
							)}
						</div>
					</motion.div>
				)}
			</AnimatePresence>

			{/* Main Content Area (Placeholder) */}
			<div className="p-6">
				<div className="flex min-h-[40rem] items-center justify-center rounded-lg border-2 border-dashed border-[var(--color-bg-alt)]">
					<p className="text-[var(--color-subtext)]">
						Configuration for {selectedDomain ?? "selected domain"} will appear
						here.
					</p>
				</div>
			</div>
		</div>
	);
}
