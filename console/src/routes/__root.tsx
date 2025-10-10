/* src/routes/__root.tsx */

import { createRootRoute, Outlet } from "@tanstack/react-router";
import { TanStackRouterDevtools } from "@tanstack/react-router-devtools";
import { Sidebar } from "~/components/sidebar/sidebar";

// Note: Framer Motion imports have been removed.

const RootLayout = () => {
	return (
		<div className="flex h-dvh">
			<Sidebar />
			<main className="flex-1 overflow-y-auto p-8 bg-[var(--color-bg-alt)]">
				{/* The animation wrappers are gone, leaving a direct Outlet. */}
				<Outlet />
			</main>
			<TanStackRouterDevtools />
		</div>
	);
};

export const Route = createRootRoute({ component: RootLayout });
