/* src/routes/instance-setup/$instanceId.tsx */

import {
	createFileRoute,
	useNavigate,
	useParams,
} from "@tanstack/react-router";
import { Split, Globe, MonitorSmartphone } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

// Define the structure for our instance data, now with OS info
interface InstanceData {
	baseUrl: string;
	os: string;
	seeds: string[];
}

// LocalStorage keys
const LS_DEFAULT_INSTANCE_KEY = "@vane/default-instance";
const LS_INSTANCES_KEY = "@vane/instance";
const LINK_EXPIRATION_MS = 5 * 60 * 1000; // 5 minutes

export const Route = createFileRoute("/instance-setup/$instanceId")({
	component: InstanceSetupComponent,
});

function InstanceSetupComponent() {
	const { instanceId } = useParams({ from: "/instance-setup/$instanceId" });
	const navigate = useNavigate();

	const [instanceData, setInstanceData] = useState<InstanceData | null>(null);
	const [error, setError] = useState<string | null>(null);
	const [isExisting, setIsExisting] = useState(false);

	// Memoize the hostname to prevent re-calculation on every render
	const hostname = useMemo(() => {
		if (!instanceData?.baseUrl) return "";
		try {
			// Use URL object to reliably extract the hostname part
			return new URL(instanceData.baseUrl).hostname;
		} catch {
			return "Invalid URL";
		}
	}, [instanceData]);

	useEffect(() => {
		try {
			const hash = window.location.hash.substring(1);
			if (!hash) {
				throw new Error("Setup token not found in URL.");
			}

			const decodedPayload = atob(hash);
			const [baseUrl, os, timestamp, ...seeds] = decodedPayload.split(";");

			if (!baseUrl || !os || !timestamp || seeds.length < 6) {
				throw new Error("Invalid setup token format.");
			}

			// 1. Validate the timestamp (within 5 minutes)
			const serverTime = new Date(timestamp).getTime();
			const clientTime = Date.now();
			if (Math.abs(clientTime - serverTime) > LINK_EXPIRATION_MS) {
				throw new Error(
					"This setup link has expired. Please generate a new one from your Vane instance."
				);
			}

			// 2. Check if this instance ID already exists in localStorage
			const allInstances = JSON.parse(
				localStorage.getItem(LS_INSTANCES_KEY) || "{}"
			);
			if (allInstances[instanceId]) {
				setIsExisting(true);
			}

			setInstanceData({ baseUrl, os, seeds });
		} catch (e) {
			console.error("Failed to parse setup token:", e);
			setError(
				e instanceof Error
					? e.message
					: "An unknown error occurred while parsing the setup link."
			);
		}
	}, [instanceId]);

	const handleSaveInstance = () => {
		if (!instanceData || !instanceId) return;

		// Set as the new default instance
		localStorage.setItem(LS_DEFAULT_INSTANCE_KEY, instanceId);

		// Add or update the instance in the main instances object
		const existingInstancesRaw = localStorage.getItem(LS_INSTANCES_KEY);
		const allInstances = existingInstancesRaw
			? JSON.parse(existingInstancesRaw)
			: {};
		allInstances[instanceId] = instanceData;
		localStorage.setItem(LS_INSTANCES_KEY, JSON.stringify(allInstances));

		// Navigate to the main dashboard
		navigate({ to: "/" });
	};

	// Base classes for buttons to keep JSX clean
	const baseButtonClasses =
		"w-full rounded-lg px-6 py-3 text-base font-medium transition hover:opacity-85";

	if (error) {
		return <SetupCard title="Setup Failed" message={error} />;
	}

	if (!instanceData) {
		return (
			<SetupCard
				title="Verifying Link..."
				message="Please wait while we validate your setup link."
			/>
		);
	}

	// Render the main confirmation UI, now with CSS variables
	return (
		<div
			className="flex h-dvh items-center justify-center p-4 font-sans"
			style={{ backgroundColor: "var(--color-bg-alt)" }}
		>
			<div
				className="w-full max-w-[420px] rounded-xl border p-8 text-center shadow-lg"
				style={{
					backgroundColor: "var(--color-bg)",
					borderColor: "var(--color-bg-alt)",
					color: "var(--color-text)",
				}}
			>
				<h1 className="mb-2 text-2xl">
					{isExisting ? "Update Vane Instance" : "Connect New Instance"}
				</h1>
				<p
					className="mb-8 leading-relaxed"
					style={{ color: "var(--color-subtext)" }}
				>
					{isExisting
						? "This instance already exists in your dashboard. Click to override its settings."
						: "A Vane instance from a new device is requesting to connect to this dashboard."}
				</p>

				<ul className="mb-8 list-none p-0 text-left">
					<li
						className="flex items-center gap-4 border-b py-3 first:pt-0 last:border-b-0 last:pb-0"
						style={{ borderColor: "var(--color-bg-alt)" }}
					>
						<div
							className="flex h-10 w-10 flex-shrink-0 items-center justify-center rounded-lg"
							style={{ backgroundColor: "var(--color-bg-alt)" }}
						>
							<Split size={20} style={{ stroke: "var(--color-subtext)" }} />
						</div>
						<div className="flex flex-col">
							<span
								className="text-sm capitalize"
								style={{ color: "var(--color-subtext)" }}
							>
								Instance ID
							</span>
							<span
								className="font-mono text-[0.9rem] font-medium"
								style={{ color: "var(--color-text)" }}
							>
								{instanceId}
							</span>
						</div>
					</li>
					<li
						className="flex items-center gap-4 border-b py-3 first:pt-0 last:border-b-0 last:pb-0"
						style={{ borderColor: "var(--color-bg-alt)" }}
					>
						<div
							className="flex h-10 w-10 flex-shrink-0 items-center justify-center rounded-lg"
							style={{ backgroundColor: "var(--color-bg-alt)" }}
						>
							<Globe size={20} style={{ stroke: "var(--color-subtext)" }} />
						</div>
						<div className="flex flex-col">
							<span
								className="text-sm capitalize"
								style={{ color: "var(--color-subtext)" }}
							>
								Address
							</span>
							<span
								className="font-mono text-[0.9rem] font-medium"
								style={{ color: "var(--color-text)" }}
							>
								{hostname}
							</span>
						</div>
					</li>
					<li
						className="flex items-center gap-4 border-b py-3 first:pt-0 last:border-b-0 last:pb-0"
						style={{ borderColor: "var(--color-bg-alt)" }}
					>
						<div
							className="flex h-10 w-10 flex-shrink-0 items-center justify-center rounded-lg"
							style={{ backgroundColor: "var(--color-bg-alt)" }}
						>
							<MonitorSmartphone
								size={20}
								style={{ stroke: "var(--color-subtext)" }}
							/>
						</div>
						<div className="flex flex-col">
							<span
								className="text-sm capitalize"
								style={{ color: "var(--color-subtext)" }}
							>
								Operating System
							</span>
							<span
								className="font-mono text-[0.9rem] font-medium"
								style={{ color: "var(--color-text)" }}
							>
								{instanceData.os}
							</span>
						</div>
					</li>
				</ul>

				<button
					className={`${baseButtonClasses} text-white`}
					style={{
						backgroundColor: isExisting ? "#dc2626" : "var(--color-primary)",
						color: isExisting ? "white" : "var(--color-bg)",
					}}
					onClick={handleSaveInstance}
				>
					{isExisting ? "Override Instance" : "Add This Instance"}
				</button>
			</div>
		</div>
	);
}

// Helper component also converted to CSS variables
function SetupCard({ title, message }: { title: string; message: string }) {
	return (
		<div
			className="flex min-h-dvh items-center justify-center p-4 font-sans"
			style={{ backgroundColor: "var(--color-bg-alt)" }}
		>
			<div
				className="w-full max-w-[420px] rounded-xl border p-8 text-center shadow-lg"
				style={{
					backgroundColor: "var(--color-bg)",
					borderColor: "var(--color-bg-alt)",
					color: "var(--color-text)",
				}}
			>
				<h1 className="mb-2 text-2xl">{title}</h1>
				<p
					className="leading-relaxed"
					style={{ color: "var(--color-subtext)" }}
				>
					{message}
				</p>
			</div>
		</div>
	);
}
