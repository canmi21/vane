/* src/routes/$instance/index.tsx */

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/$instance/")({
	component: Index,
});

function Index() {
	return null;
}
