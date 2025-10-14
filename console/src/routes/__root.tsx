/* src/routes/__root.tsx */

import { createRootRoute, Outlet, useLocation } from "@tanstack/react-router";
import { TanStackRouterDevtools } from "@tanstack/react-router-devtools";
import { Sidebar } from "~/components/sidebar/sidebar";

const RootLayout = () => {
	const location = useLocation();
	const noSidebarRoutes = ["/instance-setup/"];
	const hideSidebar = noSidebarRoutes.some((path) =>
		location.pathname.startsWith(path)
	);

	return (
		<div className="h-dvh flex">
			{!hideSidebar && <Sidebar />}
			<main className="flex-1 flex flex-col overflow-y-auto p-8 bg-[var(--color-bg-alt)]">
				<Outlet />
			</main>
			<TanStackRouterDevtools />
		</div>
	);
};

export const Route = createRootRoute({
	component: RootLayout,
});
