/* src/routes/domains/index.tsx */

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/domains/")({
	component: DomainsPage,
});

function DomainsPage() {
	return (
		<div>
			<h3 className="text-2xl font-bold">Domains</h3>
		</div>
	);
}
