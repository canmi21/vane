/* src/components/domain/canvas-connector.tsx */

interface CanvasConnectorProps {
	id: string;
	x1: number;
	y1: number;
	x2: number;
	y2: number;
	isSelected: boolean;
	onClick: (id: string) => void;
}

/**
 * A pure SVG component that draws a Bézier curve between two absolute points.
 * It now includes a hitbox for easier clicking and a selected state.
 */
export function CanvasConnector({
	id,
	x1,
	y1,
	x2,
	y2,
	isSelected,
	onClick,
}: CanvasConnectorProps) {
	const MIN_CURVATURE = 0.3;
	const MAX_CURVATURE = 0.75;
	const Y_DISTANCE_THRESHOLD = 300;
	const dy = Math.abs(y2 - y1);
	const influence = Math.min(dy, Y_DISTANCE_THRESHOLD) / Y_DISTANCE_THRESHOLD;
	const dynamicCurvature =
		MIN_CURVATURE + (MAX_CURVATURE - MIN_CURVATURE) * influence;
	const horizontalPull = Math.abs(x2 - x1) * dynamicCurvature;
	const controlX1 = x1 + horizontalPull;
	const controlY1 = y1;
	const controlX2 = x2 - horizontalPull;
	const controlY2 = y2;
	const path = `M ${x1} ${y1} C ${controlX1} ${controlY1}, ${controlX2} ${controlY2}, ${x2} ${y2}`;

	const strokeColor = isSelected
		? "var(--color-text)"
		: "var(--color-theme-border)";
	const strokeWidth = isSelected ? 3 : 2;

	return (
		<svg
			className="absolute top-0 left-0 w-px h-px overflow-visible pointer-events-auto"
			onClick={(e) => {
				e.stopPropagation();
				onClick(id);
			}}
		>
			{/* Hitbox for easier clicking */}
			<path
				d={path}
				stroke="transparent"
				strokeWidth="12"
				fill="none"
				className="cursor-pointer"
			/>
			{/* Visible Path */}
			<path
				d={path}
				stroke={strokeColor}
				strokeWidth={strokeWidth}
				fill="none"
				className="transition-[stroke,stroke-width] duration-150"
				style={{ pointerEvents: "none" }}
			/>
		</svg>
	);
}
