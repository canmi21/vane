<script lang="ts">
  import { onMount } from "svelte";
  import { getSystemInfo, listConnections } from "./lib/api";
  import type {
    SystemInfoOutput,
    ListConnectionsOutput,
  } from "./types/bindings";

  let sysInfo: SystemInfoOutput | null = $state(null);
  let connList: ListConnectionsOutput | null = $state(null);
  let error: string | null = $state(null);
  let loading = $state(false);

  async function refresh() {
    loading = true;
    error = null;
    try {
      const [sys, conns] = await Promise.all([
        getSystemInfo(),
        listConnections(),
      ]);
      sysInfo = sys;
      connList = conns;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  function formatTime(unixMs: string): string {
    return new Date(Number(unixMs)).toLocaleString();
  }

  onMount(() => {
    refresh();
  });
</script>

<main>
  <h1>Vane Console</h1>

  <button onclick={refresh} disabled={loading}>
    {loading ? "Loading..." : "Refresh"}
  </button>

  {#if error}
    <p>Error: {error}</p>
  {/if}

  {#if sysInfo}
    <section>
      <h2>System Info</h2>
      <dl>
        <dt>Version</dt>
        <dd>{sysInfo.version}</dd>
        <dt>Started at</dt>
        <dd>{formatTime(sysInfo.startedAtUnixMs)}</dd>
        <dt>Listener ports</dt>
        <dd>{sysInfo.listenerPorts.length > 0 ? sysInfo.listenerPorts.join(", ") : "None"}</dd>
        <dt>Configured ports</dt>
        <dd>{sysInfo.configuredPorts.length > 0 ? sysInfo.configuredPorts.join(", ") : "None"}</dd>
        <dt>Total connections</dt>
        <dd>{sysInfo.totalConnections}</dd>
      </dl>
    </section>
  {/if}

  <section>
    <h2>Connections</h2>
    {#if connList && connList.connections.length > 0}
      <table>
        <thead>
          <tr>
            <th>ID</th>
            <th>Client</th>
            <th>Port</th>
            <th>Layer</th>
            <th>Phase</th>
            <th>Started</th>
          </tr>
        </thead>
        <tbody>
          {#each connList.connections as conn}
            <tr>
              <td>{conn.id}</td>
              <td>{conn.peerAddr}</td>
              <td>{conn.listenPort}</td>
              <td>{conn.layer}</td>
              <td>{conn.phase}</td>
              <td>{formatTime(conn.startedAtUnixMs)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {:else}
      <p>No active connections</p>
    {/if}
  </section>
</main>
