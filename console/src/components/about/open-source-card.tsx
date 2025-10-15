/* src/components/about/open-source-card.tsx */

import { Github, BookMarked, Scale, MessagesSquare } from "lucide-react";
import { DetailCard, type DetailItem } from "./detail-card";
import { type RootInfo } from "~/routes/$instance/about/index";

export function OpenSourceCard({ rootData }: { rootData?: RootInfo | null }) {
	const items: DetailItem[] = [
		{
			icon: BookMarked,
			label: "Author",
			value: "https://github.com/canmi21",
			isLink: true,
			displayValue: "Canmi",
		},
		{
			icon: Scale,
			label: "License",
			value: rootData?.package.license || "N/A",
		},
		{
			icon: Github,
			label: "Repository",
			value: rootData?.package.repository || "N/A",
			isLink: true,
		},
		{
			icon: MessagesSquare,
			label: "Feedback",
			value: "https://github.com/canmi21/vane/issues",
			isLink: true,
		},
	];

	return <DetailCard title="Open Source" icon={Github} items={items} />;
}
