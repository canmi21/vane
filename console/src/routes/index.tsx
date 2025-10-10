/* src/routes/index.tsx */

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/")({
	component: Index,
});

function Index() {
	return (
		<div>
			{/* 使用 --color-primary 来突出标题 */}
			<h3 className="text-2xl font-bold text-[var(--color-primary)]">
				Welcome Home!
			</h3>
			{/* 使用 --color-subtext 作为次要文本 */}
			<p className="mt-2 text-[var(--color-subtext)]">
				This is the main page of your application.
			</p>
		</div>
	);
}
