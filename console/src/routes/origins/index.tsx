/* src/routes/origins/index.tsx */

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/origins/")({
	component: OriginsPage,
});

function OriginsPage() {
	return (
		<div>
			<h3 className="text-2xl font-bold">Origins</h3>
		</div>
	);
}
