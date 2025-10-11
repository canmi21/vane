/* src/routes/websocket/index.tsx */

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/websocket/")({
	component: WebSocketPage,
});

function WebSocketPage() {
	return (
		<div>
			<h3 className="text-2xl font-bold">WebSocket</h3>
		</div>
	);
}
