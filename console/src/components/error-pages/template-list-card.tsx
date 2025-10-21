/* src/components/error-pages/template-list-card.tsx */

import { useState } from "react";
import { FileCode, Plus, X, Trash2, ChevronRight } from "lucide-react";
import { motion, AnimatePresence } from "framer-motion";
import { type UseMutationResult } from "@tanstack/react-query";
import { type RequestResult } from "~/api/request";

const DEFAULT_TEMPLATE = `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Error</title>
    <style>
        body { font-family: sans-serif; text-align: center; padding: 40px; }
    </style>
</head>
<body>
    <h1>An Error Occurred</h1>
    <p>Sorry, something went wrong.</p>
</body>
</html>`;

// --- Template List Item Component ---
function TemplateItem({
	name,
	isSelected,
	onSelect,
	onDelete,
	isDeleting,
}: {
	name: string;
	isSelected: boolean;
	onSelect: () => void;
	onDelete: () => void;
	isDeleting: boolean;
}) {
	return (
		<div
			onClick={onSelect}
			className={`flex cursor-pointer items-center justify-between p-4 transition-all hover:bg-[var(--color-theme-bg)] ${
				isSelected ? "bg-[var(--color-theme-bg)]" : ""
			}`}
		>
			<div className="flex min-w-0 items-center gap-4">
				<FileCode
					size={20}
					className={
						isSelected
							? "stroke-[var(--color-theme-border)]"
							: "stroke-[var(--color-subtext)]"
					}
				/>
				<p className="truncate font-mono text-sm font-medium text-[var(--color-text)]">
					{name}.html
				</p>
			</div>
			<div className="flex flex-shrink-0 items-center gap-2">
				<button
					onClick={(e) => {
						e.stopPropagation();
						onDelete();
					}}
					disabled={isDeleting}
					className="rounded-md p-2 text-[var(--color-subtext)] transition-colors hover:text-red-500 disabled:opacity-50"
					title={`Delete ${name}.html`}
				>
					<Trash2 size={16} />
				</button>
				<ChevronRight
					size={18}
					className={`transition-transform ${isSelected ? "translate-x-1" : ""}`}
				/>
			</div>
		</div>
	);
}

// --- Main Card Component ---
export function TemplateListCard({
	templates,
	selectedTemplate,
	onSelectTemplate,
	createMutation,
	removeMutation,
}: {
	templates: string[];
	selectedTemplate: string | null;
	onSelectTemplate: (name: string | null) => void;
	createMutation: UseMutationResult<
		RequestResult<unknown>,
		Error,
		{ name: string; content: string }
	>;
	removeMutation: UseMutationResult<RequestResult<unknown>, Error, string>;
}) {
	const [isAdding, setIsAdding] = useState(false);
	const [newName, setNewName] = useState("");

	const handleCreate = (e: React.FormEvent) => {
		e.preventDefault();
		const name = newName.trim();
		if (name && /^[a-zA-Z0-9_-]+$/.test(name)) {
			createMutation.mutate(
				{ name, content: DEFAULT_TEMPLATE },
				{
					onSuccess: () => {
						setIsAdding(false);
						setNewName("");
					},
				}
			);
		} else {
			alert(
				"Name can only contain letters, numbers, hyphens, and underscores."
			);
		}
	};

	const handleDelete = (nameToDelete: string) => {
		if (
			window.confirm(`Delete "${nameToDelete}.html"? This cannot be undone.`)
		) {
			removeMutation.mutate(nameToDelete);
		}
	};

	return (
		<div className="w-full rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] shadow-sm">
			<div className="border-b border-[var(--color-bg-alt)] p-6">
				<div className="flex items-center justify-between">
					<div className="flex items-center gap-3">
						<FileCode
							size={20}
							className="stroke-[var(--color-theme-border)]"
						/>
						<h3 className="text-lg font-semibold text-[var(--color-text)]">
							Custom Error Pages
						</h3>
						<span className="rounded-md bg-[var(--color-bg-alt)] px-2 py-0.5 text-xs font-medium text-[var(--color-subtext)]">
							{templates.length}
						</span>
					</div>
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
								<Plus size={16} /> Add Page
							</>
						)}
					</button>
				</div>
			</div>

			<AnimatePresence>
				{isAdding && (
					<motion.div
						initial={{ height: 0, opacity: 0 }}
						animate={{ height: "auto", opacity: 1 }}
						exit={{ height: 0, opacity: 0 }}
						transition={{ duration: 0.3, ease: "easeInOut" }}
						className="overflow-hidden border-b border-[var(--color-bg-alt)]"
					>
						<form onSubmit={handleCreate} className="flex gap-2 p-4">
							<input
								type="text"
								value={newName}
								onChange={(e) => setNewName(e.target.value)}
								placeholder="Page name (e.g., 404, 503, maintenance)"
								className="h-10 flex-grow rounded-lg border border-[var(--color-bg-alt)] bg-[var(--color-bg-alt)] px-3 text-sm text-[var(--color-text)] placeholder-[var(--color-subtext)] transition-all focus:border-[var(--color-theme-border)] focus:outline-none focus:ring-1 focus:ring-[var(--color-theme-border)]"
								disabled={createMutation.isPending}
								required
								autoFocus
							/>
							<button
								type="submit"
								className="flex h-10 items-center gap-2 rounded-lg bg-[var(--color-theme-bg)] px-4 text-sm font-semibold text-[var(--color-text)] transition-all hover:opacity-80 disabled:cursor-not-allowed disabled:opacity-50"
								disabled={createMutation.isPending || !newName.trim()}
							>
								Create
							</button>
						</form>
						{createMutation.isError && (
							<p className="px-4 pb-2 text-xs text-red-500">
								{createMutation.error?.message || "Creation failed."}
							</p>
						)}
					</motion.div>
				)}
			</AnimatePresence>

			{/* --- FIX: Added overflow-hidden and rounding --- */}
			<div className="overflow-hidden rounded-b-xl divide-y divide-[var(--color-bg-alt)]">
				{templates.length > 0 ? (
					templates.map((name) => (
						<TemplateItem
							key={name}
							name={name}
							isSelected={selectedTemplate === name}
							onSelect={() => onSelectTemplate(name)}
							onDelete={() => handleDelete(name)}
							isDeleting={
								removeMutation.isPending && removeMutation.variables === name
							}
						/>
					))
				) : (
					<div className="p-12 text-center text-[var(--color-subtext)]">
						<p className="font-medium">No custom error pages found.</p>
						<p className="text-sm">Click "Add Page" to create one.</p>
					</div>
				)}
			</div>
		</div>
	);
}
