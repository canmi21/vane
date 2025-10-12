/* src/routes/$instance/traffic-logs/index.tsx */

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/$instance/traffic-logs/")({
	component: LogsPage,
});

function LogsPage() {
	return (
		<div>
			<h3 className="text-2xl font-bold">Logs</h3>
		</div>
	);
}
