/* src/components/domain/rate-limit-card.tsx */

import { Zap } from "lucide-react";
import { CanvasNodeCard } from "./canvas-node-card";

/**
 * A specific node card for the Rate Limit feature.
 */
export function RateLimitCard() {
	return (
		<CanvasNodeCard
			icon={Zap}
			title="Rate Limit"
			parameters={["%Request_IP%", "%Request_Domain%", "%Request_Path%"]}
			outputs={[{ label: "Accept" }, { label: "Drop" }]}
		/>
	);
}
