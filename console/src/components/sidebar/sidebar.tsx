/* src/components/sidebar/sidebar.tsx */

import { Link } from "@tanstack/react-router";
import FaviconLogo from "~/assets/favicon.svg?react";
import VaneLogo from "~/assets/vane.svg?react";

export function Sidebar() {
	return (
		// Changed background to the primary page background color.
		<aside className="w-64 h-full bg-[var(--color-bg)] px-4 py-2 flex flex-col">
			{/* App Logo container */}
			<div className="mb-6 flex justify-center items-center">
				<FaviconLogo className="h-8 w-auto" />
				<VaneLogo className="h-16 w-auto" />
			</div>

			{/* Navigation Links */}
			<nav className="flex flex-col gap-2">
				<Link
					to="/"
					className="p-2 rounded-md text-[var(--color-subtext)] hover:text-[var(--color-text)] hover:bg-[var(--color-bg-alt)] transition-colors [&.active]:font-bold [&.active]:text-[var(--color-primary)]"
				>
					Home
				</Link>
			</nav>
		</aside>
	);
}
