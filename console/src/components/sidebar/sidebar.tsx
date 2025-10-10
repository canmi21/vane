/* src/components/sidebar/sidebar.tsx */

import { Link } from "@tanstack/react-router";
import FaviconLogo from "~/assets/favicon.svg?react";
import VaneLogo from "~/assets/vane.svg?react";
import { LayoutDashboard, ShieldCheck, Globe, Blocks } from "lucide-react";

export function Sidebar() {
	const linkStyle =
		"flex items-center gap-2.5 p-2 rounded-md text-sm text-[var(--color-subtext)] hover:text-[var(--color-text)] hover:bg-[var(--color-bg-alt)] transition-colors [&.active]:font-bold [&.active]:text-[var(--color-primary)]";

	return (
		<aside className="w-64 h-full bg-[var(--color-bg)] px-4 py-2 flex flex-col">
			{/* App Logo container */}
			<div className="mb-6 flex justify-center items-center">
				<FaviconLogo className="h-8 w-auto" />
				<VaneLogo className="h-16 w-auto" />
			</div>

			{/* Navigation Links */}
			<nav className="flex flex-col gap-1">
				<Link to="/" className={linkStyle}>
					<LayoutDashboard size={18} />
					<span>Home</span>
				</Link>
				<Link to="/certificates" className={linkStyle}>
					<ShieldCheck size={18} />
					<span>Certificates</span>
				</Link>
				<Link to="/domains" className={linkStyle}>
					<Globe size={18} />
					<span>Domains</span>
				</Link>
				<Link to="/modules" className={linkStyle}>
					<Blocks size={18} />
					<span>Modules</span>
				</Link>
			</nav>
		</aside>
	);
}
