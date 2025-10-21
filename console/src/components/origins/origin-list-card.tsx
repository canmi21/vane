/* src/components/origins/origin-list-card.tsx */

import { useState } from "react";
import { Globe, Plus, X, Save, Link as LinkIcon } from "lucide-react";
import { motion, AnimatePresence } from "framer-motion";
import { type UseMutationResult } from "@tanstack/react-query";
import { type RequestResult } from "~/api/request";
import {
	type OriginResponse,
	type UpdateOriginPayload,
} from "~/routes/$instance/origins/";
import { OriginItem } from "./origin-item";

export function OriginListCard({
	origins,
	addMutation,
	updateMutation,
	removeMutation,
}: {
	origins: OriginResponse[];
	addMutation: UseMutationResult<RequestResult<OriginResponse>, Error, string>;
	updateMutation: UseMutationResult<
		RequestResult<OriginResponse>,
		Error,
		{ id: string; payload: UpdateOriginPayload }
	>;
	removeMutation: UseMutationResult<RequestResult<unknown>, Error, string>;
}) {
	const [newOriginUrl, setNewOriginUrl] = useState("");
	const [isAddFormExpanded, setIsAddFormExpanded] = useState(false);

	const handleAddOrigin = (e: React.FormEvent) => {
		e.preventDefault();
		if (newOriginUrl.trim()) {
			addMutation.mutate(newOriginUrl.trim(), {
				onSuccess: () => setNewOriginUrl(""),
			});
		}
	};

	return (
		// --- FIX: Removed rounding from main container ---
		<div className="border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-sm rounded-xl">
			{/* Header with title --- FIX: Added top rounding --- */}
			<div className="border-b border-[var(--color-bg-alt)] p-6">
				<div className="flex items-center justify-between">
					<div className="flex items-center gap-3">
						<Globe size={20} className="stroke-[var(--color-theme-border)]" />
						<h3 className="text-lg font-semibold text-[var(--color-text)]">
							Configured Origins
						</h3>
						<span className="rounded-md bg-[var(--color-bg-alt)] px-2 py-0.5 text-xs font-medium text-[var(--color-subtext)]">
							{origins.length}
						</span>
					</div>
					<button
						onClick={() => setIsAddFormExpanded(!isAddFormExpanded)}
						className="flex items-center gap-2 rounded-lg border-2 border-[var(--color-theme-border)] bg-[var(--color-theme-bg)] px-3 py-1.5 text-sm font-semibold text-[var(--color-text)] transition-all hover:opacity-80"
					>
						{isAddFormExpanded ? (
							<>
								<X size={16} />
								Cancel
							</>
						) : (
							<>
								<Plus size={16} />
								Add Origin
							</>
						)}
					</button>
				</div>
			</div>

			{/* Collapsible add form */}
			<AnimatePresence>
				{isAddFormExpanded && (
					<motion.div
						initial={{ height: 0, opacity: 0 }}
						animate={{ height: "auto", opacity: 1 }}
						exit={{ height: 0, opacity: 0 }}
						transition={{ duration: 0.3, ease: "easeInOut" }}
						className="overflow-hidden border-b border-[var(--color-bg-alt)]"
					>
						<div className="p-4">
							<form onSubmit={handleAddOrigin} className="flex gap-2">
								<input
									type="text"
									value={newOriginUrl}
									onChange={(e) => setNewOriginUrl(e.target.value)}
									placeholder="http(s)://{host}:{port}/{path}"
									className="flex-grow rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg-alt)] px-3 py-2 text-sm text-[var(--color-text)] placeholder-[var(--color-subtext)] transition-all focus:border-[var(--color-theme-border)] focus:outline-none focus:ring-1 focus:ring-[var(--color-theme-border)]"
									disabled={addMutation.isPending}
									autoFocus
								/>
								<button
									type="submit"
									className="flex items-center gap-2 rounded-lg border-2 border-[var(--color-theme-border)] bg-[var(--color-theme-bg)] px-4 py-2 text-sm font-semibold text-[var(--color-text)] transition-all hover:opacity-80 disabled:cursor-not-allowed disabled:opacity-50"
									disabled={addMutation.isPending || !newOriginUrl.trim()}
								>
									<Save size={16} />
									Save
								</button>
							</form>
							{addMutation.isError && (
								<p className="mt-2 text-xs text-red-500">
									{addMutation.error?.message || "Failed to add origin."}
								</p>
							)}
						</div>
					</motion.div>
				)}
			</AnimatePresence>

			{/* Origins list --- FIX: Added overflow-hidden and bottom rounding --- */}
			<div className="overflow-hidden rounded-b-xl divide-y divide-[var(--color-bg-alt)]">
				{origins.length > 0 ? (
					origins.map((item) => (
						<OriginItem
							key={item.id}
							item={item}
							updateMutation={updateMutation}
							removeMutation={removeMutation}
						/>
					))
				) : (
					<div className="flex flex-col items-center gap-4 p-12 text-center text-[var(--color-subtext)]">
						<LinkIcon size={32} />
						<p className="font-medium">No origins configured.</p>
						<p className="text-sm">Click "Add Origin" above to get started.</p>
					</div>
				)}
			</div>
		</div>
	);
}
