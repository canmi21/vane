import type {
  ListConnectionsOutput,
  SystemInfoOutput,
  GetConfigOutput,
  UpdateConfigInput,
  UpdateConfigOutput,
} from "../types/bindings";

const BASE = "/_bridge";

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`${BASE}${path}`, init);
  if (!res.ok) {
    throw new Error(`${res.status} ${res.statusText}`);
  }
  return res.json();
}

export function listConnections(): Promise<ListConnectionsOutput> {
  return request("/listConnections");
}

export function getSystemInfo(): Promise<SystemInfoOutput> {
  return request("/getSystemInfo");
}

export function getConfig(): Promise<GetConfigOutput> {
  return request("/getConfig");
}

export function updateConfig(
  input: UpdateConfigInput,
): Promise<UpdateConfigOutput> {
  return request("/updateConfig", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
}
