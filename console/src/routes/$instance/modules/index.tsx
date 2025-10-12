/* src/routes/$instance/modules/index.tsx */

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/$instance/modules/")({
	component: ModulesPage,
});

function ModulesPage() {
	return (
		<div>
			<h3 className="text-2xl font-bold">Modules</h3>
		</div>
	);
}
