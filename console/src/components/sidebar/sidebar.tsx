/* src/components/sidebar/sidebar.tsx */

import { Link, useRouterState, useNavigate } from "@tanstack/react-router";
import { useState, useEffect } from "react";
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
	PanelRightClose,
	PanelRightOpen,
} from "lucide-react";
import { motion, AnimatePresence } from "framer-motion";

const navLinks = [
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

// --- Framer Motion Variants ---
const sidebarVariants = {
	expanded: { width: "16rem" },
	collapsed: { width: "4.5rem" },
};

const labelVariants = {
	hidden: { opacity: 0, x: -10 },
	visible: { opacity: 1, x: 0 },
};

// --- FINAL FIX: Animation for the header content ---
const headerContentVariants = {
	hidden: { opacity: 0, scale: 0.9 },
	visible: { opacity: 1, scale: 1 },
};

export function Sidebar() {
	const { location } = useRouterState();
	const navigate = useNavigate();

	const [isCollapsed, setIsCollapsed] = useState(() => {
		const savedState = localStorage.getItem("@vane/sidebar");
		return savedState ? JSON.parse(savedState) : false;
	});

	useEffect(() => {
		localStorage.setItem("@vane/sidebar", JSON.stringify(isCollapsed));
	}, [isCollapsed]);

	const instance = location.pathname.split("/")[1];

	useEffect(() => {
		if (!instance && location.pathname !== "/") {
			navigate({ to: "/" });
		}
	}, [instance, location.pathname, navigate]);

	if (!instance) {
		return null;
	}

	const buttonClasses =
		"p-1 rounded-md text-[var(--color-subtext)] hover:bg-[var(--color-bg-alt)] hover:text-[var(--color-text)] transition-colors";

	return (
		<motion.aside
			variants={sidebarVariants}
			initial={false}
			animate={isCollapsed ? "collapsed" : "expanded"}
			transition={{ duration: 0.3, ease: "easeInOut" }}
			className="h-full bg-[var(--color-bg)] px-4 py-2 flex flex-col border-r border-[var(--color-bg-alt)]"
		>
			{/* --- FINAL FIX: Conditionally render the entire header content for a cleaner transition --- */}
			<div
				className={`mb-2 flex h-16 items-center ${
					isCollapsed ? "justify-center" : "justify-between"
				}`}
			>
				<AnimatePresence initial={false} mode="wait">
					{isCollapsed ? (
						<motion.div
							key="collapsed-header"
							variants={headerContentVariants}
							initial="hidden"
							animate="visible"
							exit="hidden"
							transition={{ duration: 0.2 }}
						>
							<button
								onClick={() => setIsCollapsed(false)}
								className={buttonClasses}
							>
								<PanelRightClose size={20} />
							</button>
						</motion.div>
					) : (
						<motion.div
							key="expanded-header"
							variants={headerContentVariants}
							initial="hidden"
							animate="visible"
							exit="hidden"
							transition={{ duration: 0.2 }}
							className="flex w-full items-center justify-between"
						>
							<Link
								to="/$instance/home"
								params={{ instance }}
								className="flex items-center overflow-hidden"
							>
								<FaviconLogo className="h-8 w-8 flex-shrink-0" />
								<VaneLogo className="h-16 w-auto" />
							</Link>
							<button
								onClick={() => setIsCollapsed(true)}
								className={buttonClasses}
							>
								<PanelRightOpen size={20} />
							</button>
						</motion.div>
					)}
				</AnimatePresence>
			</div>

			{/* Navigation section remains the same */}
			<nav className="flex-1 flex flex-col gap-1 overflow-y-auto pb-2">
				{navLinks.map(({ to, label, Icon }) => {
					const targetPath = `/${instance}${to}`;
					const isActive = location.pathname.startsWith(targetPath);

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

							<div
								className={`col-start-1 row-start-1 flex items-center gap-2.5 p-2 ${
									isCollapsed ? "justify-center" : ""
								}`}
							>
								<Icon
									size={18}
									className={`flex-shrink-0 transition-colors ${
										isActive ? "text-[var(--color-text)]" : ""
									}`}
								/>
								<AnimatePresence>
									{!isCollapsed && (
										<motion.span
											variants={labelVariants}
											initial="hidden"
											animate="visible"
											exit="hidden"
											transition={{ duration: 0.2, delay: 0.1 }}
											className={`whitespace-nowrap ${
												isActive ? "text-[var(--color-text)]" : ""
											}`}
										>
											{label}
										</motion.span>
									)}
								</AnimatePresence>
							</div>
						</Link>
					);
				})}
			</nav>
		</motion.aside>
	);
}
