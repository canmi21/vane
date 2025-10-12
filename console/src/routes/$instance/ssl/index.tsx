/* src/routes/$instance/ssl/index.tsx */

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/$instance/ssl/")({
	component: CertificatesPage,
});

function CertificatesPage() {
	return (
		<div>
			<h3 className="text-2xl font-bold">Certificates</h3>
		</div>
	);
}
