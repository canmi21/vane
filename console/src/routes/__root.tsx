/* src/routes/__root.tsx */

import { createRootRoute, Outlet } from "@tanstack/react-router";
import { TanStackRouterDevtools } from "@tanstack/react-router-devtools";
import { Sidebar } from "~/components/sidebar/sidebar";

const RootLayout = () => (
	<div className="flex h-dvh">
		<Sidebar />
		<main className="flex-1 overflow-y-auto p-8 bg-[var(--color-bg-alt)]">
			<Outlet />
		</main>

		<TanStackRouterDevtools />
	</div>
);

export const Route = createRootRoute({ component: RootLayout });
