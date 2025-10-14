/* src/routes/$instance/home/index.tsx */

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/$instance/home/")({
	component: HomePage,
});

function HomePage() {
	return (
		<div>
			<h3 className="text-2xl font-bold">Home Pages</h3>
		</div>
	);
}
