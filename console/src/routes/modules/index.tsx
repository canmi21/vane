/* src/routes/modules/index.tsx */

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/modules/")({
	component: ModulesPage,
});

function ModulesPage() {
	return (
		<div>
			<h3 className="text-2xl font-bold">Modules</h3>
		</div>
	);
}
