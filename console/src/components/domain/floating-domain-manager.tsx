/* src/components/domain/floating-domain-manager.tsx */

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

// A compact, floating component for managing domain selection, addition, and removal.
export function FloatingDomainManager({
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

	// --- Event Handlers ---

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

	// --- Render ---

	return (
		<div className="fixed bottom-6 right-6 z-50 flex flex-col items-end gap-3">
			{/* Collapsible Add Domain Form */}
			<AnimatePresence>
				{isAdding && (
					<motion.div
						initial={{ opacity: 0, y: 10 }}
						animate={{ opacity: 1, y: 0 }}
						exit={{ opacity: 0, y: 10 }}
						transition={{ duration: 0.2, ease: "easeInOut" }}
					>
						<div className="w-80 rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] p-4 shadow-lg">
							<form onSubmit={handleAddDomain} className="flex flex-col gap-2">
								<input
									type="text"
									value={newDomain}
									onChange={(e) => setNewDomain(e.target.value)}
									placeholder="example.com or *.example.com"
									className="h-10 w-full rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg-alt)] px-3 text-sm text-[var(--color-text)] placeholder-[var(--color-subtext)] transition-all focus:border-[var(--color-theme-border)] focus:outline-none focus:ring-1 focus:ring-[var(--color-theme-border)]"
									disabled={addMutation.isPending}
									autoFocus
								/>
								<button
									type="submit"
									className="flex h-10 w-full items-center justify-center gap-2 rounded-lg bg-[var(--color-theme-bg)] px-4 text-sm font-semibold text-[var(--color-text)] transition-all hover:opacity-80 disabled:cursor-not-allowed disabled:opacity-50"
									disabled={addMutation.isPending || !newDomain.trim()}
								>
									<Save size={16} />
									<span>Add Domain</span>
								</button>
								{addMutation.isError && (
									<p className="mt-1 text-xs text-red-500">
										{addMutation.error?.message || "Failed to add domain."}
									</p>
								)}
							</form>
						</div>
					</motion.div>
				)}
			</AnimatePresence>

			{/* Main Floating Control Bar */}
			<div className="flex h-14 w-auto items-center gap-2 rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] p-2 shadow-lg">
				{/* Domain Selector */}
				<Select.Root
					value={selectedDomain ?? ""}
					onValueChange={setSelectedDomain}
					disabled={isAdding}
				>
					<Select.Trigger className="flex h-10 w-56 items-center justify-between rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg-alt)] px-3 text-sm text-[var(--color-text)] transition-all hover:border-[var(--color-theme-border)] focus:border-[var(--color-theme-border)] focus:outline-none focus:ring-1 focus:ring-[var(--color-theme-border)] disabled:cursor-not-allowed disabled:opacity-50">
						<div className="flex items-center gap-2 overflow-hidden">
							<Network size={16} className="stroke-[var(--color-subtext)]" />
							<span className="truncate">
								<Select.Value placeholder="Select a domain..." />
							</span>
						</div>
						<Select.Icon>
							<ChevronDown size={16} />
						</Select.Icon>
					</Select.Trigger>
					<Select.Portal>
						<Select.Content
							position="popper"
							side="top"
							align="end"
							sideOffset={10}
							className="z-50 w-[--radix-select-trigger-width] overflow-hidden rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-lg"
						>
							<Select.Viewport className="p-1">
								{domains.map((domain) => (
									<Select.Item
										key={domain}
										value={domain}
										className="relative my-0.5 flex cursor-pointer select-none items-center justify-between rounded-md py-1.5 pl-8 pr-2 text-sm text-[var(--color-text)] outline-none hover:bg-[var(--color-theme-bg)] data-[state=checked]:bg-[var(--color-theme-bg)]"
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

				{/* Action Buttons */}
				<button
					onClick={handleRemoveSelectedDomain}
					disabled={!selectedDomain || removeMutation.isPending || isAdding}
					className="flex h-10 w-10 items-center justify-center rounded-lg text-[var(--color-subtext)] transition-all hover:bg-[var(--color-bg-alt)] hover:text-red-500 disabled:cursor-not-allowed disabled:opacity-50"
					title={
						selectedDomain ? `Delete ${selectedDomain}` : "No domain selected"
					}
				>
					<Trash2 size={16} />
				</button>

				<button
					onClick={() => setIsAdding(!isAdding)}
					className={`flex h-10 w-10 items-center justify-center rounded-lg text-sm font-semibold text-[var(--color-text)] transition-all ${
						isAdding
							? "bg-red-500/20 text-red-500 hover:bg-red-500/30"
							: "bg-[var(--color-theme-bg)] hover:opacity-80"
					}`}
				>
					{isAdding ? <X size={16} /> : <Plus size={16} />}
				</button>
			</div>
		</div>
	);
}
