/* src/routes/__root.tsx */

import { createRootRoute, Link, Outlet } from "@tanstack/react-router";
import { TanStackRouterDevtools } from "@tanstack/react-router-devtools";

const RootLayout = () => (
	// This will now work because main.tsx imports the CSS
	<main className="bg-[var(--color-bg)] text-[var(--color-text)] min-h-screen">
		{/* Navigation Bar */}
		<nav className="p-4 flex gap-4 border-b border-b-[var(--color-bg-alt)]">
			<Link
				to="/"
				className="text-[var(--color-subtext)] hover:text-[var(--color-text)] transition-colors [&.active]:font-bold [&.active]:text-[var(--color-primary)]"
			>
				Home
			</Link>
			<Link
				to="/about"
				className="text-[var(--color-subtext)] hover:text-[var(--color-text)] transition-colors [&.active]:font-bold [&.active]:text-[var(--color-primary)]"
			>
				About
			</Link>
		</nav>

		{/* Page Content */}
		<div className="p-4">
			<Outlet />
		</div>

		<TanStackRouterDevtools />
	</main>
);

export const Route = createRootRoute({ component: RootLayout });
