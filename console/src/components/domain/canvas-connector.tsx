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
	// --- MODIFIED: The curvature is now dynamic based on the vertical distance between points. ---
	// This makes the curve feel more natural and less "stiff".

	// --- Tuning Knobs ---
	// The curvature when Y distance is 0. A lower value makes the curve tighter.
	const MIN_CURVATURE = 0.3;
	// The curvature when Y distance is at or beyond the threshold. This is the max "S" shape.
	const MAX_CURVATURE = 0.75;
	// The vertical distance (in pixels) at which the curve reaches its maximum curvature.
	const Y_DISTANCE_THRESHOLD = 300;

	// Calculate the vertical distance and determine the influence factor (0.0 to 1.0).
	const dy = Math.abs(y2 - y1);
	const influence = Math.min(dy, Y_DISTANCE_THRESHOLD) / Y_DISTANCE_THRESHOLD;

	// Linearly interpolate the curvature based on the influence factor.
	const dynamicCurvature =
		MIN_CURVATURE + (MAX_CURVATURE - MIN_CURVATURE) * influence;

	// The horizontal "pull" of the control points, now driven by the dynamic curvature.
	const horizontalPull = Math.abs(x2 - x1) * dynamicCurvature;

	// The first control point is always to the right of the start point.
	const controlX1 = x1 + horizontalPull;
	const controlY1 = y1;

	// The second control point is always to the left of the end point.
	const controlX2 = x2 - horizontalPull;
	const controlY2 = y2;

	const path = `M ${x1} ${y1} C ${controlX1} ${controlY1}, ${controlX2} ${controlY2}, ${x2} ${y2}`;

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
