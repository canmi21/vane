/* src/routes/$instance/error-pages/index.tsx */

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/$instance/error-pages/")({
	component: ErrorPage,
});

function ErrorPage() {
	return (
		<div>
			<h3 className="text-2xl font-bold">Error Pages</h3>
		</div>
	);
}
