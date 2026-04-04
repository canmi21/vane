<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { getSystemInfo } from "../lib/api";
  import type { SystemInfoOutput } from "../types/bindings";

  let info: SystemInfoOutput | null = $state(null);
  let error: string | null = $state(null);
  let initialLoading = $state(true);

  async function load() {
    try {
      info = await getSystemInfo();
      error = null;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      initialLoading = false;
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

  let timer: ReturnType<typeof setInterval>;
  onMount(() => { load(); timer = setInterval(load, 5000); });
  onDestroy(() => { clearInterval(timer); });
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

  {#if initialLoading}
    <p class="text-nord-3 text-sm">Loading...</p>
  {:else if info}
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
        <div class="text-xs uppercase tracking-wider text-nord-3 mb-1">Active Listeners</div>
        <div class="text-lg font-mono text-frost-cyan">{info.activeListeners}</div>
      </div>
      <div class="bg-nord-1 rounded-lg p-4">
        <div class="text-xs uppercase tracking-wider text-nord-3 mb-1">Connections</div>
        <div class="text-lg font-mono text-frost-cyan">{info.totalConnections}</div>
      </div>
    </div>

    <div class="text-xs text-nord-3">
      {info.configuredRules} listener rule{info.configuredRules === 1 ? "" : "s"} configured.
      Started at {formatTime(info.startedAtUnixMs)}.
    </div>
  {/if}
</div>
