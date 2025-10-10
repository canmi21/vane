/* src/routes/cors/index.tsx */

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/cors/")({
	component: CorsPage,
});

function CorsPage() {
	return (
		<div>
			<h3 className="text-2xl font-bold">CORS</h3>
		</div>
	);
}
