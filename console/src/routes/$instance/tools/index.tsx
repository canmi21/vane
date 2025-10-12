/* src/routes/$instance/tools/index.tsx */

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/$instance/tools/")({
	component: ToolsPage,
});

function ToolsPage() {
	return (
		<div>
			<h3 className="text-2xl font-bold">Tools</h3>
		</div>
	);
}
