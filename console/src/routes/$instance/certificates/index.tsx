/* src/routes/$instance/certificates/index.tsx */

import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/$instance/certificates/")({
	component: CertificatesPage,
});

function CertificatesPage() {
	return (
		<div>
			<h3 className="text-2xl font-bold">Certificates</h3>
		</div>
	);
}
