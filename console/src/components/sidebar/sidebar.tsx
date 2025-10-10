/* src/components/sidebar/sidebar.tsx */

import { Link } from "@tanstack/react-router";

export function Sidebar() {
	return (
		// Sidebar container: fixed width, full height, and distinct background.
		<aside className="w-64 h-full bg-[var(--color-bg-alt)] p-4 flex flex-col">
			{/* App Title or Logo */}
			<div className="mb-8">
				<h1 className="text-2xl font-bold text-[var(--color-primary)]">Vane</h1>
			</div>

			{/* Navigation Links */}
			<nav className="flex flex-col gap-2">
				<Link
					to="/"
					className="p-2 rounded-md text-[var(--color-subtext)] hover:text-[var(--color-text)] hover:bg-[var(--color-bg)] transition-colors [&.active]:font-bold [&.active]:text-[var(--color-primary)]"
				>
					Home
				</Link>
				{/* Add more links here in the future */}
			</nav>
		</aside>
	);
}
