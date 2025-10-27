/* src/components/domain/canvas-connector.tsx */

/**
 * Renders a simple, straight SVG line to connect two nodes.
 * It's positioned absolutely relative to the starting node's container.
 */
export function CanvasConnector({
	width,
	height,
}: {
	width: number;
	height: number;
}) {
	// --- MODIFIED: The line's starting point is offset by 4px (half the handle's width) ---
	// This prevents it from drawing over the starting handle, making the connection look clean.
	const pathData = `M 4,${height / 2} L ${width},${height / 2}`;

	return (
		<svg
			width={width}
			height={height}
			className="absolute top-1/2 -translate-y-1/2 left-full overflow-visible pointer-events-none"
		>
			<path
				d={pathData}
				stroke="var(--color-theme-border)"
				strokeWidth="2"
				fill="none"
			/>
		</svg>
	);
}
