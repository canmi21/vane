/* src/routes/cache/index.tsx */

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/cache/")({
	component: CachePage,
});

function CachePage() {
	return (
		<div>
			<h3 className="text-2xl font-bold">Cache</h3>
		</div>
	);
}
