/* src/components/about/detail-card.tsx */

import React from "react";

export type DetailItem = {
	icon: React.ElementType;
	label: string;
	value: string;
	isLink?: boolean;
	displayValue?: string;
};

export function DetailCard({
	title,
	icon: Icon,
	items,
}: {
	title: string;
	icon: React.ElementType;
	items: DetailItem[];
}) {
	return (
		<div className="rounded-xl border border-[var(--color-bg-alt)] bg-[var(--color-bg)] p-6 shadow-sm transition-all hover:shadow-md">
			<div className="mb-4 flex items-center gap-3">
				<Icon size={20} className="stroke-[var(--color-theme-border)]" />
				<h3 className="text-lg font-semibold text-[var(--color-text)]">
					{title}
				</h3>
			</div>
			<div className="space-y-3">
				{items.map((item, idx) => (
					<div key={idx} className="flex min-h-[36px] items-center gap-4">
						<div className="flex h-full items-center">
							<item.icon
								size={20}
								className="flex-shrink-0 stroke-[var(--color-subtext)]"
							/>
						</div>
						<div className="flex flex-col">
							<span className="text-xs text-[var(--color-subtext)]">
								{item.label}
							</span>
							{item.isLink ? (
								<a
									href={item.value}
									target="_blank"
									rel="noopener noreferrer"
									className="break-all font-mono text-sm font-medium text-[var(--color-theme-border)] hover:underline"
								>
									{item.displayValue || item.value.replace(/^https?:\/\//, "")}
								</a>
							) : (
								<span className="font-mono text-sm font-medium text-[var(--color-text)]">
									{item.value}
								</span>
							)}
						</div>
					</div>
				))}
			</div>
		</div>
	);
}
