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
	FilePenLine,
	Info,
	CircleSlash,
} from "lucide-react";
import { motion } from "framer-motion";

const navLinks = [
	// Keep these paths relative, we'll add the instance prefix dynamically.
	{ to: "/", label: "Dashboard", Icon: LayoutDashboard },
	{ to: "/origins", label: "Origin Server", Icon: Server },
	{ to: "/domains", label: "Domains", Icon: Globe },
	{ to: "/certificates", label: "SSL Certificates", Icon: ShieldCheck },
	{ to: "/error-pages", label: "Error Pages", Icon: CircleSlash },
	{ to: "/cache-control", label: "Cache Control", Icon: Archive },
	{ to: "/cors-management", label: "CORS", Icon: ArrowRightLeft },
	{ to: "/custom-header", label: "Header Override", Icon: FilePenLine },
	{ to: "/rate-limit", label: "Rate Limit", Icon: Zap },
	{ to: "/websocket", label: "WebSocket", Icon: RadioTower },
	{ to: "/traffic-logs", label: "Traffic Logs", Icon: FileText },
	{ to: "/modules", label: "Modules", Icon: Blocks },
	{ to: "/tools", label: "Tools", Icon: Wrench },
	{ to: "/about", label: "About", Icon: Info },
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
