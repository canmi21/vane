<script lang="ts">
  import { onMount } from "svelte";
  import { getConfig, getSystemInfo, updateConfig } from "../lib/api";

  // Runtime shape of the config JSON (JsonBlob is an actual object despite TS typing it as string)
  interface PortEntry {
    port: number;
    targetIp: string;
    targetPort: number;
    listening: boolean;
  }

  let ports: PortEntry[] = $state([]);
  let error: string | null = $state(null);
  let saving = $state(false);

  // Add-port form
  let showForm = $state(false);
  let formPort = $state(8080);
  let formTargetIp = $state("127.0.0.1");
  let formTargetPort = $state(3000);

  async function load() {
    error = null;
    try {
      const [configResult, sysInfo] = await Promise.all([
        getConfig(),
        getSystemInfo(),
      ]);
      // config is a JsonBlob (actual JSON object at runtime)
      const config = configResult.config as unknown as Record<string, unknown>;
      const portsMap = (config.ports ?? {}) as Record<string, Record<string, unknown>>;
      const listeningSet = new Set(sysInfo.listenerPorts);

      ports = Object.entries(portsMap).map(([key, pc]) => {
        const portNum = Number(key);
        const target = (pc.target ?? {}) as Record<string, unknown>;
        return {
          port: portNum,
          targetIp: String(target.ip ?? ""),
          targetPort: Number(target.port ?? 0),
          listening: listeningSet.has(portNum),
        };
      });
      ports.sort((a, b) => a.port - b.port);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function addPort() {
    saving = true;
    error = null;
    try {
      const configResult = await getConfig();
      const config = configResult.config as unknown as Record<string, unknown>;
      const portsMap = ((config.ports ?? {}) as Record<string, unknown>);

      portsMap[String(formPort)] = {
        listen: {},
        target: { ip: formTargetIp, port: formTargetPort },
      };

      config.ports = portsMap;
      const result = await updateConfig({ config: config as unknown as string });
      if (!result.ok) {
        error = result.error ?? result.validationErrors.map((v) => v.message).join("; ");
        return;
      }
      showForm = false;
      await load();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      saving = false;
    }
  }

  async function removePort(port: number) {
    saving = true;
    error = null;
    try {
      const configResult = await getConfig();
      const config = configResult.config as unknown as Record<string, unknown>;
      const portsMap = ((config.ports ?? {}) as Record<string, unknown>);

      delete portsMap[String(port)];
      config.ports = portsMap;

      const result = await updateConfig({ config: config as unknown as string });
      if (!result.ok) {
        error = result.error ?? result.validationErrors.map((v) => v.message).join("; ");
        return;
      }
      await load();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      saving = false;
    }
  }

  onMount(() => { load(); });
</script>

<div>
  <div class="flex items-center justify-between mb-6">
    <h1 class="text-2xl font-semibold text-nord-6">Ports</h1>
    <div class="flex gap-2">
      <button
        class="px-3 py-1.5 text-sm bg-nord-2 hover:bg-nord-3 text-nord-4 rounded transition-colors"
        onclick={load}
      >Refresh</button>
      <button
        class="px-3 py-1.5 text-sm bg-frost-deep hover:bg-frost-blue text-nord-6 rounded transition-colors"
        onclick={() => (showForm = !showForm)}
      >{showForm ? "Cancel" : "Add Port"}</button>
    </div>
  </div>

  {#if error}
    <div class="p-3 rounded bg-nord-1 border border-aurora-red/40 text-aurora-red text-sm mb-4">
      {error}
    </div>
  {/if}

  {#if showForm}
    <div class="bg-nord-1 rounded-lg p-5 mb-6 border border-nord-2">
      <h2 class="text-sm font-semibold text-nord-5 mb-4 uppercase tracking-wider">New L4 TCP Forward</h2>
      <div class="grid grid-cols-3 gap-4">
        <div>
          <label class="block text-xs text-nord-3 mb-1" for="listen-port">Listen Port</label>
          <input
            id="listen-port"
            type="number"
            min="1"
            max="65535"
            bind:value={formPort}
            class="w-full px-3 py-2 bg-nord-0 border border-nord-3 rounded text-nord-4 font-mono text-sm focus:border-frost-cyan focus:outline-none"
          />
        </div>
        <div>
          <label class="block text-xs text-nord-3 mb-1" for="target-ip">Target IP</label>
          <input
            id="target-ip"
            type="text"
            bind:value={formTargetIp}
            class="w-full px-3 py-2 bg-nord-0 border border-nord-3 rounded text-nord-4 font-mono text-sm focus:border-frost-cyan focus:outline-none"
          />
        </div>
        <div>
          <label class="block text-xs text-nord-3 mb-1" for="target-port">Target Port</label>
          <input
            id="target-port"
            type="number"
            min="1"
            max="65535"
            bind:value={formTargetPort}
            class="w-full px-3 py-2 bg-nord-0 border border-nord-3 rounded text-nord-4 font-mono text-sm focus:border-frost-cyan focus:outline-none"
          />
        </div>
      </div>
      <button
        class="mt-4 px-4 py-2 text-sm bg-aurora-green hover:bg-aurora-green/80 text-nord-0 font-medium rounded transition-colors disabled:opacity-50"
        disabled={saving}
        onclick={addPort}
      >{saving ? "Saving..." : "Save"}</button>
    </div>
  {/if}

  {#if ports.length > 0}
    <div class="bg-nord-1 rounded-lg overflow-hidden">
      <table class="w-full text-sm">
        <thead>
          <tr class="border-b border-nord-2 text-xs uppercase tracking-wider text-nord-3">
            <th class="text-left px-4 py-3 font-medium">Port</th>
            <th class="text-left px-4 py-3 font-medium">Forward Target</th>
            <th class="text-left px-4 py-3 font-medium">Status</th>
            <th class="text-right px-4 py-3 font-medium">Actions</th>
          </tr>
        </thead>
        <tbody>
          {#each ports as p}
            <tr class="border-b border-nord-2/50 hover:bg-nord-2/30 transition-colors">
              <td class="px-4 py-3 font-mono text-frost-blue">{p.port}</td>
              <td class="px-4 py-3 font-mono text-nord-4">{p.targetIp}:{p.targetPort}</td>
              <td class="px-4 py-3">
                {#if p.listening}
                  <span class="inline-flex items-center gap-1.5 text-aurora-green text-xs">
                    <span class="w-1.5 h-1.5 rounded-full bg-aurora-green"></span>
                    Listening
                  </span>
                {:else}
                  <span class="inline-flex items-center gap-1.5 text-nord-3 text-xs">
                    <span class="w-1.5 h-1.5 rounded-full bg-nord-3"></span>
                    Stopped
                  </span>
                {/if}
              </td>
              <td class="px-4 py-3 text-right">
                <button
                  class="px-2 py-1 text-xs bg-aurora-red/20 hover:bg-aurora-red/40 text-aurora-red rounded transition-colors disabled:opacity-50"
                  disabled={saving}
                  onclick={() => removePort(p.port)}
                >Delete</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {:else}
    <div class="bg-nord-1 rounded-lg p-8 text-center">
      <p class="text-nord-3 mb-2">No ports configured</p>
      <p class="text-nord-3/60 text-sm">Click "Add Port" to create a L4 TCP forward rule.</p>
    </div>
  {/if}
</div>
