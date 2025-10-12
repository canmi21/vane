/* src/routes/$instance/about/index.tsx */

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/$instance/about/")({
	component: AboutPage,
});

function AboutPage() {
	return (
		<div>
			<h3 className="text-2xl font-bold">About</h3>
		</div>
	);
}
