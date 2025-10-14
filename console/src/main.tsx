/* src/main.tsx */

import { StrictMode } from "react";
import ReactDOM from "react-dom/client";
import { RouterProvider, createRouter } from "@tanstack/react-router";
import { routeTree } from "./routeTree.gen";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import "~/styles/essential.css";
import "~/styles/colors.css";
import "~/styles/element.css";
import "~/styles/transitions.css";
import "~/styles/tailwindcss.css";

// Add 'dark' class to <html>
document.documentElement.classList.add("dark");

// Create a new router instance
const router = createRouter({ routeTree });

// Create a client for React Query
const queryClient = new QueryClient();

// Register the router instance for type safety
declare module "@tanstack/react-router" {
	interface Register {
		router: typeof router;
	}
}

// Render the app
const rootElement = document.getElementById("root")!;
if (!rootElement.innerHTML) {
	const root = ReactDOM.createRoot(rootElement);
	root.render(
		<StrictMode>
			<QueryClientProvider client={queryClient}>
				<RouterProvider router={router} />
			</QueryClientProvider>
		</StrictMode>
	);
}
