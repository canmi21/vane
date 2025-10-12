/* src/routes/$instance/custom-header/index.tsx */

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/$instance/custom-header/")({
	component: HeaderPage,
});

function HeaderPage() {
	return (
		<div>
			<h3 className="text-2xl font-bold">Header</h3>
		</div>
	);
}
