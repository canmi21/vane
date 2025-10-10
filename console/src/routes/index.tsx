/* src/routes/index.tsx */

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/")({
	component: Index,
});

function Index() {
	return (
		<div>
			<h3 className="text-2xl font-bold text-[var(--color-primary)]">
				Welcome Home!
			</h3>
			<p className="mt-2 text-[var(--color-subtext)]">
				This is the main page of your application. The sidebar is fixed, and
				this content area will scroll if it's too long.
			</p>

			{/* Long content to test the independent scrolling */}
			<div className="mt-12 space-y-4">
				<h4 className="text-lg font-semibold">Scroll Test Section</h4>
				{Array.from({ length: 25 }).map((_, i) => (
					<div
						key={i}
						className="p-4 rounded-lg border border-[var(--color-bg-alt)]"
					>
						<p className="font-mono text-[var(--color-tertiary)]">
							Content block #{i + 1}
						</p>
						<p>
							This is a placeholder to make the content long enough to trigger
							the scrollbar on the main content panel.
						</p>
					</div>
				))}
			</div>
		</div>
	);
}
