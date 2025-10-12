/* src/components/sidebar/sidebar.tsx */

import { Link, useRouterState, useNavigate } from "@tanstack/react-router";
import { useEffect } from "react";
import FaviconLogo from "~/assets/favicon.svg?react";
import VaneLogo from "~/assets/vane.svg?react";
import {
	LayoutDashboard,
	ShieldCheck,
	Globe,
	Blocks,
	Zap,
	ArrowRightLeft,
	Server,
	Archive,
	FileText,
	RadioTower,
	Wrench,
} from "lucide-react";
import { motion } from "framer-motion";

const navLinks = [
	// Keep these paths relative, we'll add the instance prefix dynamically.
	{ to: "/", label: "Home", Icon: LayoutDashboard },
	{ to: "/domains", label: "Domains", Icon: Globe },
	{ to: "/origins", label: "Origins", Icon: Server },
	{ to: "/ssl", label: "SSL", Icon: ShieldCheck },
	{ to: "/cache", label: "Cache", Icon: Archive },
	{ to: "/logs", label: "Logs", Icon: FileText },
	{ to: "/cors", label: "CORS", Icon: ArrowRightLeft },
	{ to: "/ratelimit", label: "Rate Limit", Icon: Zap },
	{ to: "/websocket", label: "WebSocket", Icon: RadioTower },
	{ to: "/modules", label: "Modules", Icon: Blocks },
	{ to: "/tools", label: "Tools", Icon: Wrench },
];

export function Sidebar() {
	const { location } = useRouterState();
	const navigate = useNavigate();

	// Get the instance directly from the URL pathname.
	// e.g., for "/abcde/domains", this will be "abcde".
	const instance = location.pathname.split("/")[1];

	// If the instance is missing from the URL for any reason,
	// navigate to the root. The root route will then handle
	// creating/finding an instance and redirecting back correctly.
	useEffect(() => {
		if (!instance && location.pathname !== "/") {
			navigate({ to: "/" });
		}
	}, [instance, location.pathname, navigate]);

	// If there is no instance, we can't build the links.
	// Return null to avoid rendering a broken sidebar while the redirect happens.
	if (!instance) {
		return null;
	}

	return (
		<aside className="w-64 h-full bg-[var(--color-bg)] px-4 py-2 flex flex-col">
			{/* This part remains fixed at the top. */}
			{/* The 'instance' is now a guaranteed string, so this works. */}
			<Link
				to="/$instance"
				params={{ instance }}
				className="mb-2 flex justify-center items-center"
			>
				<FaviconLogo className="h-8 w-auto" />
				<VaneLogo className="h-16 w-auto" />
			</Link>

			{/* This nav is now the scrollable area. */}
			{/* flex-1 makes it take all available vertical space. */}
			{/* overflow-y-auto shows a scrollbar only when needed. */}
			<nav className="flex-1 flex flex-col gap-1 overflow-y-auto pb-2">
				{navLinks.map(({ to, label, Icon }) => {
					// Construct the full path with the instance parameter.
					const targetPath = to === "/" ? `/${instance}` : `/${instance}${to}`;
					const isActive = location.pathname === targetPath;

					return (
						<Link
							key={to}
							to={targetPath}
							className="grid rounded-md text-sm text-[var(--color-subtext)] transition-colors hover:text-[var(--color-text)]"
						>
							{isActive && (
								<motion.div
									layoutId="active-indicator"
									className="col-start-1 row-start-1 h-full w-full rounded-md bg-[var(--color-theme-bg)] border border-[var(--color-theme-border)]"
								/>
							)}

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
