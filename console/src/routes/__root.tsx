/* src/routes/__root.tsx */

import { createRootRoute, Outlet, useLocation } from "@tanstack/react-router";
import { TanStackRouterDevtools } from "@tanstack/react-router-devtools";
import { Sidebar } from "~/components/sidebar/sidebar";

const RootLayout = () => {
	const location = useLocation();

	// --- Condition to hide the sidebar ---
	const noSidebarRoutes = ["/instance-setup/"];
	const hideSidebar = noSidebarRoutes.some((path) =>
		location.pathname.startsWith(path)
	);

	// --- Condition for full-screen canvas pages ---
	const isCanvasPage = location.pathname.includes("/domains");

	// --- Dynamically set main element classes based on the route ---
	const mainContainerClasses = isCanvasPage
		? "relative flex-1 flex flex-col overflow-hidden" // No padding, overflow hidden for canvas
		: "flex-1 flex flex-col overflow-y-auto p-8 bg-[var(--color-bg-alt)]"; // Default layout with padding

	return (
		<div className="h-dvh flex bg-[var(--color-bg-alt)]">
			{!hideSidebar && <Sidebar />}
			<main className={mainContainerClasses}>
				<Outlet />
			</main>
			<TanStackRouterDevtools />
		</div>
	);
};

export const Route = createRootRoute({
	component: RootLayout,
});
