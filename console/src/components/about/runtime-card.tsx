/* src/components/about/runtime-card.tsx */

import { Layers, Cpu, Code, Terminal, Clock } from "lucide-react";
import { DetailCard, type DetailItem } from "./detail-card";
import { type RootInfo } from "~/routes/$instance/about/index";

// --- Helper Functions ---
const parseToolVersion = (toolString: string = "") =>
	toolString.split(" ")[1] ?? "N/A";
const formatDateTime = (dateString: string) => {
	try {
		return new Date(dateString).toLocaleString("en-US", {
			year: "numeric",
			month: "short",
			day: "numeric",
			hour: "2-digit",
			minute: "2-digit",
		});
	} catch {
		return "N/A";
	}
};

export function RuntimeCard({ rootData }: { rootData?: RootInfo | null }) {
	const items: DetailItem[] = [
		{
			icon: Cpu,
			label: "Platform",
			value: `${rootData?.runtime.platform} (${rootData?.runtime.arch})`,
		},
		{
			icon: Code,
			label: "Rust",
			value: parseToolVersion(rootData?.build.rust),
		},
		{
			icon: Terminal,
			label: "Cargo",
			value: parseToolVersion(rootData?.build.cargo),
		},
		{
			icon: Clock,
			label: "Server Time",
			value: formatDateTime(rootData?.timestamp || ""),
		},
	];

	return <DetailCard title="Runtime Environment" icon={Layers} items={items} />;
}
