/* src/routes/__root.tsx */

import { createRootRoute, Outlet } from "@tanstack/react-router";
import { TanStackRouterDevtools } from "@tanstack/react-router-devtools";
import { Sidebar } from "~/components/sidebar/sidebar";

const RootLayout = () => (
	// Main app container: uses Flexbox and is fixed to the viewport height.
	<div className="flex h-dvh bg-[var(--color-bg)] text-[var(--color-text)]">
		{/* Sidebar Component */}
		<Sidebar />

		{/* Main content area */}
		{/* Takes remaining space (flex-1) and enables vertical scrolling only for itself. */}
		<main className="flex-1 overflow-y-auto p-8">
			<Outlet /> {/* Page content (like index.tsx) will be rendered here */}
		</main>

		<TanStackRouterDevtools />
	</div>
);

export const Route = createRootRoute({ component: RootLayout });
