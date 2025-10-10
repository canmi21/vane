/* src/components/sidebar/sidebar.tsx */

import { Link, useRouterState } from "@tanstack/react-router";
import FaviconLogo from "~/assets/favicon.svg?react";
import VaneLogo from "~/assets/vane.svg?react";
import {
	LayoutDashboard,
	ShieldCheck,
	Globe,
	Blocks,
	Zap,
	ArrowRightLeft,
} from "lucide-react";
import { motion } from "framer-motion";

const navLinks = [
	{ to: "/", label: "Home", Icon: LayoutDashboard },
	{ to: "/domains", label: "Domains", Icon: Globe },
	{ to: "/ssl", label: "SSL", Icon: ShieldCheck },
	{ to: "/cors", label: "CORS", Icon: ArrowRightLeft },
	{ to: "/ratelimit", label: "Rate Limit", Icon: Zap },
	{ to: "/modules", label: "Modules", Icon: Blocks },
];

export function Sidebar() {
	const { location } = useRouterState();

	return (
		<aside className="w-64 h-full bg-[var(--color-bg)] px-4 py-2 flex flex-col">
			<div className="mb-6 flex justify-center items-center">
				<FaviconLogo className="h-8 w-auto" />
				<VaneLogo className="h-16 w-auto" />
			</div>

			<nav className="flex flex-col gap-1">
				{navLinks.map(({ to, label, Icon }) => {
					const isActive = location.pathname === to;
					return (
						<Link
							key={to}
							to={to}
							// The base style for the link container.
							className="relative flex items-center gap-2.5 p-2 rounded-md text-sm text-[var(--color-subtext)] transition-colors hover:text-[var(--color-text)]"
						>
							{/* This is the animated active state indicator. */}
							{isActive && (
								<motion.div
									layoutId="active-indicator"
									// New styling with semi-transparent background and solid border.
									className="absolute inset-0 rounded-md z-[-1] bg-[rgb(var(--color-theme))]/10 border border-[rgb(var(--color-theme))]"
								/>
							)}

							{/* Icon and Text: color changes when active. */}
							<Icon
								size={18}
								className={`transition-colors ${
									isActive ? "text-[rgb(var(--color-theme))]" : ""
								}`}
							/>
							<span
								className={`transition-colors ${
									isActive ? "text-[rgb(var(--color-theme))]" : ""
								}`}
							>
								{label}
							</span>
						</Link>
					);
				})}
			</nav>
		</aside>
	);
}
