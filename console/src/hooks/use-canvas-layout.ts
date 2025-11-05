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

	// This function now handles adding ANY plugin-based node, including terminal ones.
	const addNode = useCallback(
		(plugin: Plugin) => {
			if (!layout) return;
			const defaultData: Record<string, unknown> = {};

			// Set default values based on parameter type
			for (const key in plugin.input_params) {
				const param = plugin.input_params[key];
				if (param.type === "number") defaultData[key] = 0;
				else if (param.type === "boolean") defaultData[key] = false;
				else defaultData[key] = "";
			}

			const newNode: CanvasNode = {
				id: nanoid(8),
				type: plugin.name,
				version: plugin.version, // Store the version of the plugin
				x: 350,
				y: 350,
				// Terminal nodes (return: true) have no outputs, so their 'tree' is empty.
				// This automatically handles creating an empty `outputs` array for them.
				inputs: plugin.output_results.return
					? [{ id: "input", label: "Input" }]
					: [{ id: "input", label: "Input" }],
				outputs: plugin.output_results.tree.map((handle) => ({
					id: handle,
					label: handle.charAt(0).toUpperCase() + handle.slice(1),
				})),
				data: defaultData,
				variables: plugin.output_results.variables,
			};
			handleLayoutChange({ ...layout, nodes: [...layout.nodes, newNode] });
		},
		[layout, handleLayoutChange]
	);

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
		updateNodeData,
		syncStatus,
	};
}
