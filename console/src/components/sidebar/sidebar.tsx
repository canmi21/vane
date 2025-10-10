/* src/components/sidebar/sidebar.tsx */

import { Link } from "@tanstack/react-router";
import VaneLogo from "~/assets/vane.svg?react";

export function Sidebar() {
	return (
		// Reduced vertical padding from p-4 to py-2 to decrease top/bottom spacing.
		<aside className="w-64 h-full bg-[var(--color-bg-alt)] px-4 py-2 flex flex-col">
			{/* App Logo container */}
			{/* Reduced margin-bottom to better fit the new padding. */}
			<div className="mb-6 flex justify-center">
				{/* Kept the logo size at h-16 as requested. */}
				<VaneLogo className="h-16 w-auto" />
			</div>

			{/* Navigation Links */}
			<nav className="flex flex-col gap-2">
				<Link
					to="/"
					className="p-2 rounded-md text-[var(--color-subtext)] hover:text-[var(--color-text)] hover:bg-[var(--color-bg)] transition-colors [&.active]:font-bold [&.active]:text-[var(--color-primary)]"
				>
					Home
				</Link>
			</nav>
		</aside>
	);
}
