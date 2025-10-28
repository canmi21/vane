/* src/components/domain/canvas-connector.tsx */

interface CanvasConnectorProps {
	x1: number;
	y1: number;
	x2: number;
	y2: number;
}

/**
 * A pure SVG component that draws a Bézier curve between two absolute points within the canvas coordinate space.
 */
export function CanvasConnector({ x1, y1, x2, y2 }: CanvasConnectorProps) {
	// Control points create a pleasing horizontal "S" curve.
	const controlX1 = x1 + Math.abs(x2 - x1) / 2;
	const controlY1 = y1;
	const controlX2 = x2 - Math.abs(x2 - x1) / 2;
	const controlY2 = y2;

	const path = `M ${x1} ${y1} C ${controlX1} ${controlY1}, ${controlX2} ${controlY2}, ${x2} ${y2}`;

	// This SVG sits inside the main scaled/panned div, so its coordinates are relative to that space.
	return (
		<svg className="absolute top-0 left-0 w-px h-px overflow-visible pointer-events-none">
			<path
				d={path}
				stroke="var(--color-theme-border)"
				strokeWidth="2"
				fill="none"
			/>
		</svg>
	);
}
