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
	{ to: "/home", label: "Dashboard", Icon: LayoutDashboard },
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

	const instance = location.pathname.split("/")[1];

	useEffect(() => {
		if (!instance && location.pathname !== "/") {
			navigate({ to: "/" });
		}
	}, [instance, location.pathname, navigate]);

	if (!instance) {
		return null;
	}

	return (
		<aside className="w-64 h-full bg-[var(--color-bg)] px-4 py-2 flex flex-col border-r border-[var(--color-bg-alt)]">
			<Link
				to="/$instance/home"
				params={{ instance }}
				className="mb-2 flex justify-center items-center"
			>
				<FaviconLogo className="h-8 w-auto" />
				<VaneLogo className="h-16 w-auto" />
			</Link>

			<nav className="flex-1 flex flex-col gap-1 overflow-y-auto pb-2">
				{navLinks.map(({ to, label, Icon }) => {
					const targetPath = to === "/" ? `/${instance}` : `/${instance}${to}`;

					const isActive =
						location.pathname === targetPath ||
						location.pathname.startsWith(`${targetPath}/`);

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
