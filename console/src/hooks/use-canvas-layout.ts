/* src/hooks/use-canvas-layout.ts */

import { useState, useEffect, useCallback } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import {
	getLayoutConfig,
	updateLayoutConfig,
	saveLayoutToLocalStorage,
	type CanvasLayout,
	type CanvasNode,
	type EntryPointNodeData,
	type ErrorPageNodeData, // --- FINAL FIX: Import the new data type ---
} from "~/lib/canvas-layout";
import { nanoid } from "nanoid";
import { type Plugin } from "./use-plugin-data";

// --- Types ---
export type SyncStatus =
	| "unloaded"
	| "loading"
	| "saved"
	| "saving"
	| "unsaved"
	| "error";

interface UseCanvasLayoutProps {
	instanceId: string;
	selectedDomain: string | null;
}

const AUTOSAVE_DELAY = 1000; // 1 second

/**
 * Manages the canvas layout state, including fetching from and saving to the backend,
 * using LocalStorage as an intermediary cache.
 */
export function useCanvasLayout({
	instanceId,
	selectedDomain,
}: UseCanvasLayoutProps) {
	const queryClient = useQueryClient();
	const [layout, setLayout] = useState<CanvasLayout | null>(null);
	const [syncStatus, setSyncStatus] = useState<SyncStatus>("unloaded");

	const generateDefaultLayout = useCallback((): CanvasLayout => {
		const entryPointNode: CanvasNode<EntryPointNodeData> = {
			id: "entry-point",
			type: "entry-point",
			x: 150,
			y: 200,
			inputs: [],
			outputs: [{ id: "output", label: "Output" }],
			data: {},
		};
		return { nodes: [entryPointNode], connections: [] };
	}, []);

	// --- Data Fetching from Backend ---
	const layoutQuery = useQuery({
		queryKey: ["layout", instanceId, selectedDomain],
		queryFn: () => getLayoutConfig(instanceId, selectedDomain!),
		enabled: !!selectedDomain,
	});

	// --- Data Saving to Backend ---
	const saveMutation = useMutation({
		mutationFn: (newLayout: CanvasLayout) =>
			updateLayoutConfig(instanceId, selectedDomain!, newLayout),
		onMutate: () => setSyncStatus("saving"),
		onSuccess: () => setSyncStatus("saved"),
		onError: () => setSyncStatus("error"),
	});

	// --- Effect to synchronize Backend -> LocalStorage -> UI State ---
	useEffect(() => {
		if (layoutQuery.isLoading) {
			setSyncStatus("loading");
		} else if (layoutQuery.isSuccess && selectedDomain) {
			const backendData = layoutQuery.data?.data;
			// Check if the backend returned an empty object `{}`.
			if (backendData && Object.keys(backendData).length === 0) {
				const newLayout = generateDefaultLayout();
				setLayout(newLayout);
				saveLayoutToLocalStorage(selectedDomain, newLayout);
				setSyncStatus("unsaved"); // Mark as unsaved to trigger the first save.
			}
			// If the backend returned a valid layout.
			else if (backendData) {
				setLayout(backendData);
				saveLayoutToLocalStorage(selectedDomain, backendData);
				setSyncStatus("saved");
			}
		} else if (layoutQuery.isError) {
			setSyncStatus("error");
		} else if (!selectedDomain) {
			setLayout(null);
			setSyncStatus("unloaded");
		}
	}, [
		selectedDomain,
		layoutQuery.isLoading,
		layoutQuery.isSuccess,
		layoutQuery.isError,
		layoutQuery.data?.data,
		generateDefaultLayout,
	]);

	// --- Debounced autosave effect ---
	useEffect(() => {
		if (syncStatus === "unsaved" && layout) {
			const handler = setTimeout(() => {
				saveMutation.mutate(layout);
			}, AUTOSAVE_DELAY);
			return () => clearTimeout(handler);
		}
	}, [layout, syncStatus, saveMutation]);

	// This is the central point for all local modifications.
	const handleLayoutChange = useCallback(
		(newLayout: CanvasLayout) => {
			if (selectedDomain) {
				setLayout(newLayout);
				saveLayoutToLocalStorage(selectedDomain, newLayout);
				setSyncStatus("unsaved");
			}
		},
		[selectedDomain]
	);

	// The functions below now call the centralized `handleLayoutChange`.
	const addNode = useCallback(
		(plugin: Plugin) => {
			if (!layout) return;
			const defaultData: Record<string, unknown> = {};
			for (const key in plugin.input_params) {
				const param = plugin.input_params[key];
				if (param.type === "number")
					defaultData[key] = key.includes("requests") ? 100 : 0;
				else if (param.type === "boolean") defaultData[key] = false;
				else defaultData[key] = "";
			}
			const newNode: CanvasNode = {
				id: nanoid(8),
				type: plugin.name,
				x: 350,
				y: 350,
				inputs: [{ id: "input", label: "Input" }],
				outputs: plugin.output_results.tree.map((handle) => ({
					id: handle,
					label: handle.charAt(0).toUpperCase() + handle.slice(1),
				})),
				data: defaultData,
			};
			handleLayoutChange({ ...layout, nodes: [...layout.nodes, newNode] });
		},
		[layout, handleLayoutChange]
	);

	// --- FINAL FIX: Add a dedicated function to create an Error Page node. ---
	const addErrorPageNode = useCallback(() => {
		if (!layout) return;

		const defaultData: ErrorPageNodeData = {
			status_code: 500,
			status_description: "Internal Server Error",
			reason: "An internal error occurred on the server.",
			request_id: "{{req.id}}",
			timestamp: "{{req.timestamp}}",
			version: "{{vane.version}}",
			request_ip: "{{req.ip}}",
			visitor_tip: "Please try again later or contact support.",
			admin_guide: "Check service logs for detailed error information.",
		};

		const newNode: CanvasNode<ErrorPageNodeData> = {
			id: nanoid(8),
			type: "error-page",
			x: 350,
			y: 350,
			inputs: [{ id: "input", label: "Input" }],
			outputs: [], // No outputs for this node type
			data: defaultData,
		};

		handleLayoutChange({ ...layout, nodes: [...layout.nodes, newNode] });
	}, [layout, handleLayoutChange]);

	const updateNodeData = useCallback(
		(nodeId: string, newData: Record<string, unknown>) => {
			if (!layout) return;
			const newNodes = layout.nodes.map((n) =>
				n.id === nodeId ? { ...n, data: newData } : n
			);
			handleLayoutChange({ ...layout, nodes: newNodes });
		},
		[layout, handleLayoutChange]
	);

	// When switching domains, invalidate the query to force a re-fetch.
	useEffect(() => {
		if (selectedDomain) {
			queryClient.invalidateQueries({
				queryKey: ["layout", instanceId, selectedDomain],
			});
		}
	}, [selectedDomain, instanceId, queryClient]);

	return {
		layout,
		handleLayoutChange,
		addNode,
		addErrorPageNode, // Export the new function
		updateNodeData,
		syncStatus,
	};
}
