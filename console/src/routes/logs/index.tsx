/* src/routes/logs/index.tsx */

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/logs/")({
	component: LogsPage,
});

function LogsPage() {
	return (
		<div>
			<h3 className="text-2xl font-bold">Logs</h3>
		</div>
	);
}
