<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { listConnections } from "../lib/api";
  import type { ListConnectionsOutput } from "../types/bindings";

  let data: ListConnectionsOutput | null = $state(null);
  let error: string | null = $state(null);
  let initialLoading = $state(true);

  async function load() {
    try {
      data = await listConnections();
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

  function phaseColor(phase: string): string {
    switch (phase) {
      case "Forwarding": return "text-aurora-green";
      default: return "text-nord-4";
    }
  }

  let timer: ReturnType<typeof setInterval>;
  onMount(() => { load(); timer = setInterval(load, 2000); });
  onDestroy(() => { clearInterval(timer); });
</script>

<div>
  <div class="flex items-center justify-between mb-6">
    <h1 class="text-2xl font-semibold text-nord-6">Connections</h1>
    <button
      class="px-3 py-1.5 text-sm bg-nord-2 hover:bg-nord-3 text-nord-4 rounded transition-colors disabled:opacity-50"
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
  {:else if data && data.connections.length > 0}
    <div class="text-xs text-nord-3 mb-3">{data.total} active connection{data.total === 1 ? "" : "s"}</div>
    <div class="bg-nord-1 rounded-lg overflow-hidden">
      <table class="w-full text-sm">
        <thead>
          <tr class="border-b border-nord-2 text-xs uppercase tracking-wider text-nord-3">
            <th class="text-left px-4 py-3 font-medium">ID</th>
            <th class="text-left px-4 py-3 font-medium">Client</th>
            <th class="text-left px-4 py-3 font-medium">Port</th>
            <th class="text-left px-4 py-3 font-medium">Phase</th>
            <th class="text-left px-4 py-3 font-medium">Started</th>
          </tr>
        </thead>
        <tbody>
          {#each data.connections as conn}
            <tr class="border-b border-nord-2/50 hover:bg-nord-2/30 transition-colors">
              <td class="px-4 py-3 font-mono text-xs text-nord-3" title={conn.id}>{conn.id.slice(0, 8)}</td>
              <td class="px-4 py-3 font-mono text-nord-4">{conn.peerAddr}</td>
              <td class="px-4 py-3 font-mono text-frost-blue">{conn.listenPort}</td>
              <td class="px-4 py-3 text-xs {phaseColor(conn.phase)}">{conn.phase}</td>
              <td class="px-4 py-3 text-xs text-nord-3">{formatTime(conn.startedAtUnixMs)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {:else if data}
    <div class="bg-nord-1 rounded-lg p-8 text-center">
      <p class="text-nord-3 mb-2">No active connections</p>
      <p class="text-nord-3/60 text-sm">Connections will appear here when clients connect to configured ports.</p>
    </div>
  {/if}
</div>
