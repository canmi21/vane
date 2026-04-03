<script lang="ts">
  import { onMount } from "svelte";
  import { getSystemInfo } from "../lib/api";
  import type { SystemInfoOutput } from "../types/bindings";

  let info: SystemInfoOutput | null = $state(null);
  let error: string | null = $state(null);

  async function load() {
    error = null;
    try {
      info = await getSystemInfo();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  function formatTime(unixMs: string): string {
    return new Date(Number(unixMs)).toLocaleString();
  }

  function uptime(startedAtUnixMs: string): string {
    const diff = Date.now() - Number(startedAtUnixMs);
    const seconds = Math.floor(diff / 1000);
    if (seconds < 60) return `${seconds}s`;
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
    const hours = Math.floor(minutes / 60);
    return `${hours}h ${minutes % 60}m`;
  }

  onMount(() => { load(); });
</script>

<div>
  <div class="flex items-center justify-between mb-6">
    <h1 class="text-2xl font-semibold text-nord-6">Overview</h1>
    <button
      class="px-3 py-1.5 text-sm bg-nord-2 hover:bg-nord-3 text-nord-4 rounded transition-colors"
      onclick={load}
    >Refresh</button>
  </div>

  {#if error}
    <div class="p-3 rounded bg-nord-1 border border-aurora-red/40 text-aurora-red text-sm mb-4">
      {error}
    </div>
  {/if}

  {#if info}
    <div class="grid grid-cols-4 gap-4 mb-8">
      <div class="bg-nord-1 rounded-lg p-4">
        <div class="text-xs uppercase tracking-wider text-nord-3 mb-1">Version</div>
        <div class="text-lg font-mono text-frost-cyan">{info.version}</div>
      </div>
      <div class="bg-nord-1 rounded-lg p-4">
        <div class="text-xs uppercase tracking-wider text-nord-3 mb-1">Uptime</div>
        <div class="text-lg font-mono text-frost-cyan">{uptime(info.startedAtUnixMs)}</div>
      </div>
      <div class="bg-nord-1 rounded-lg p-4">
        <div class="text-xs uppercase tracking-wider text-nord-3 mb-1">Listening Ports</div>
        <div class="text-lg font-mono text-frost-cyan">{info.listenerPorts.length}</div>
      </div>
      <div class="bg-nord-1 rounded-lg p-4">
        <div class="text-xs uppercase tracking-wider text-nord-3 mb-1">Connections</div>
        <div class="text-lg font-mono text-frost-cyan">{info.totalConnections}</div>
      </div>
    </div>

    <h2 class="text-lg font-semibold text-nord-5 mb-3">Listening Ports</h2>
    {#if info.listenerPorts.length > 0}
      <div class="flex flex-wrap gap-2">
        {#each info.listenerPorts as port}
          <span class="px-3 py-1 bg-nord-1 rounded font-mono text-sm text-frost-blue">
            :{port}
          </span>
        {/each}
      </div>
    {:else}
      <p class="text-nord-3 text-sm">No ports currently listening.</p>
    {/if}

    <h2 class="text-lg font-semibold text-nord-5 mb-3 mt-6">Configured Ports</h2>
    {#if info.configuredPorts.length > 0}
      <div class="flex flex-wrap gap-2">
        {#each info.configuredPorts as port}
          <span class="px-3 py-1 bg-nord-1 rounded font-mono text-sm text-nord-4">
            :{port}
          </span>
        {/each}
      </div>
    {:else}
      <p class="text-nord-3 text-sm">No ports configured.</p>
    {/if}

    <div class="mt-6 text-xs text-nord-3">
      Started at {formatTime(info.startedAtUnixMs)}
    </div>
  {/if}
</div>
