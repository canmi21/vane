/* src/routes/ratelimit/index.tsx */

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/ratelimit/")({
	component: RateLimitPage,
});

function RateLimitPage() {
	return (
		<div>
			<h3 className="text-2xl font-bold">Rate Limit</h3>
		</div>
	);
}
