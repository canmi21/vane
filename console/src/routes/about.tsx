/* src/routes/about.tsx */

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/about")({
	component: About,
});

function About() {
	return (
		<div>
			<h3 className="text-2xl font-bold text-[var(--color-primary)]">
				About Us
			</h3>
			<p className="mt-2 text-[var(--color-subtext)]">
				This page provides information about the project.
			</p>
		</div>
	);
}
