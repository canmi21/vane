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
						// THE FIX IS HERE: The Link is now a single-cell grid container.
						<Link
							key={to}
							to={to}
							className="grid rounded-md text-sm text-[var(--color-subtext)] transition-colors hover:text-[var(--color-text)]"
						>
							{/* The background, which sits in the single grid cell. */}
							{isActive && (
								<motion.div
									layoutId="active-indicator"
									className="col-start-1 row-start-1 h-full w-full rounded-md bg-[var(--color-theme-bg)] border border-[var(--color-theme-border)]"
								/>
							)}

							{/* The content (icon + text), which sits in the same cell, on top of the background. */}
							<div className="col-start-1 row-start-1 flex items-center gap-2.5 p-2">
								<Icon
									size={18}
									className={`transition-colors ${
										isActive ? "text-[var(--color-text)]" : ""
									}`}
								/>
								<span
									className={`transition-colors ${
										isActive ? "text-[var(--color-text)]" : ""
									}`}
								>
									{label}
								</span>
							</div>
						</Link>
					);
				})}
			</nav>
		</aside>
	);
}
