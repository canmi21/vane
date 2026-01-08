/* components/mermaid.tsx */

'use client';
import { type MermaidConfig, default as mermaid } from 'mermaid';
import { useTheme } from 'next-themes';
import { useEffect, useId, useRef, useState, useSyncExternalStore } from 'react';

export type MermaidProps = {
	chart: string;
	config?: MermaidConfig;
};

// Safe way to detect client-side rendering without cascading renders via useEffect
const emptySubscribe = () => () => {};
const useIsClient = () => {
	return useSyncExternalStore(
		emptySubscribe,
		() => true,
		() => false,
	);
};

export function Mermaid({ chart, config }: MermaidProps) {
	const id = useId();
	const [svg, setSvg] = useState<string>('');
	const isClient = useIsClient();
	const { resolvedTheme } = useTheme();
	const containerRef = useRef<HTMLDivElement>(null);

	useEffect(() => {
		// Capture ref in a local variable to satisfy TypeScript
		const container = containerRef.current;
		if (!isClient || !container) return;

		const renderChart = async () => {
			try {
				mermaid.initialize({
					startOnLoad: false,
					theme: resolvedTheme === 'dark' ? 'dark' : 'default',
					suppressErrorRendering: true,
					...config,
				});

				const { svg: renderedSvg } = await mermaid.render(
					`mermaid-${id.replace(/:/g, '')}`,
					chart,
					container,
				);
				setSvg(renderedSvg);
			} catch (error) {
				console.error('Mermaid render error:', error);
			}
		};

		renderChart();
	}, [resolvedTheme, isClient, chart, config, id]);

	// SSR Placeholder
	if (!isClient) {
		return <div className="mermaid-loading animate-pulse h-64 bg-secondary/20 rounded-lg" />;
	}

	return (
		<div
			ref={containerRef}
			className="mermaid-diagram flex justify-center p-4 bg-background rounded-lg overflow-x-auto"
			dangerouslySetInnerHTML={{ __html: svg }}
		/>
	);
}
