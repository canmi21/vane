<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { getConfig, updateConfig, compileListeners } from "../lib/api";
  import type {
    ListenerRule,
    Protocol,
    CompiledListener,
  } from "../types/bindings";
  import { z } from "zod/v4";

  // -- Zod validation schema for the add-rule form -------------------------

  const portSpec = z.string().check(
    z.refine((v) => {
      const match = v.match(/^(\d+)(?:-(\d+))?$/);
      if (!match) return false;
      const start = Number(match[1]);
      const end = match[2] ? Number(match[2]) : start;
      return start >= 1 && start <= 65535 && end >= 1 && end <= 65535 && start <= end;
    }, "Must be a port (1-65535) or range (start-end)")
  );

  const ipAddr = z.string().check(
    z.refine((v) => {
      // Simple IPv4 + IPv6 check
      if (v === "0.0.0.0" || v === "::") return true;
      const ipv4 = /^(\d{1,3}\.){3}\d{1,3}$/.test(v) && v.split(".").every((o) => Number(o) <= 255);
      const ipv6 = v.includes(":");
      return ipv4 || ipv6;
    }, "Must be a valid IP address")
  );

  const ruleSchema = z.object({
    bind: ipAddr,
    port: portSpec,
    protocol: z.enum(["tcp", "udp", "both"]),
  });

  // -- State ---------------------------------------------------------------

  let rules: ListenerRule[] = $state([]);
  let compiled: CompiledListener[] = $state([]);
  let compileError: string | null = $state(null);
  let error: string | null = $state(null);
  let saving = $state(false);
  let initialLoading = $state(true);

  // Form
  let showForm = $state(false);
  let formBind = $state("0.0.0.0");
  let formPort = $state("8080");
  let formProtocol: Protocol = $state("tcp");
  let formErrors: string[] = $state([]);

  // -- Load / refresh ------------------------------------------------------

  async function load() {
    try {
      const configResult = await getConfig();
      const config = configResult.config as unknown as Record<string, unknown>;
      rules = ((config.listeners ?? []) as ListenerRule[]);
      await refreshCompile();
      error = null;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      initialLoading = false;
    }
  }

  async function refreshCompile() {
    try {
      const result = await compileListeners({ listeners: rules });
      if (result.ok) {
        compiled = result.listeners;
        compileError = null;
      } else {
        compiled = [];
        compileError = result.error;
      }
    } catch (e) {
      compiled = [];
      compileError = e instanceof Error ? e.message : String(e);
    }
  }

  // -- Add / Remove --------------------------------------------------------

  function validateForm(): boolean {
    const result = ruleSchema.safeParse({
      bind: formBind,
      port: formPort,
      protocol: formProtocol,
    });
    if (!result.success) {
      formErrors = result.error.issues.map((i) => `${i.path.join(".")}: ${i.message}`);
      return false;
    }
    formErrors = [];
    return true;
  }

  async function addRule() {
    if (!validateForm()) return;
    saving = true;
    error = null;
    try {
      const configResult = await getConfig();
      const config = configResult.config as unknown as Record<string, unknown>;
      const listeners = ((config.listeners ?? []) as ListenerRule[]);
      listeners.push({ bind: formBind, port: formPort, protocol: formProtocol });
      config.listeners = listeners;

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

  async function removeRule(index: number) {
    saving = true;
    error = null;
    try {
      const configResult = await getConfig();
      const config = configResult.config as unknown as Record<string, unknown>;
      const listeners = ((config.listeners ?? []) as ListenerRule[]);
      listeners.splice(index, 1);
      config.listeners = listeners;

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

  let timer: ReturnType<typeof setInterval>;
  onMount(() => { load(); timer = setInterval(load, 5000); });
  onDestroy(() => { clearInterval(timer); });
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
        onclick={() => { showForm = !showForm; formErrors = []; }}
      >{showForm ? "Cancel" : "Add Rule"}</button>
    </div>
  </div>

  {#if error}
    <div class="p-3 rounded bg-nord-1 border border-aurora-red/40 text-aurora-red text-sm mb-4">
      {error}
    </div>
  {/if}

  <!-- Add Rule Form -->
  {#if showForm}
    <div class="bg-nord-1 rounded-lg p-5 mb-6 border border-nord-2">
      <h2 class="text-sm font-semibold text-nord-5 mb-4 uppercase tracking-wider">New Listener Rule</h2>
      {#if formErrors.length > 0}
        <div class="p-2 rounded bg-aurora-red/10 border border-aurora-red/30 text-aurora-red text-xs mb-3">
          {#each formErrors as fe}
            <div>{fe}</div>
          {/each}
        </div>
      {/if}
      <div class="grid grid-cols-4 gap-4">
        <div>
          <label class="block text-xs text-nord-3 mb-1" for="rule-bind">Bind Address</label>
          <input
            id="rule-bind"
            type="text"
            bind:value={formBind}
            class="w-full px-3 py-2 bg-nord-0 border border-nord-3 rounded text-nord-4 font-mono text-sm focus:border-frost-cyan focus:outline-none"
          />
        </div>
        <div>
          <label class="block text-xs text-nord-3 mb-1" for="rule-port">Port / Range</label>
          <input
            id="rule-port"
            type="text"
            bind:value={formPort}
            placeholder="8080 or 8000-8100"
            class="w-full px-3 py-2 bg-nord-0 border border-nord-3 rounded text-nord-4 font-mono text-sm focus:border-frost-cyan focus:outline-none"
          />
        </div>
        <div>
          <label class="block text-xs text-nord-3 mb-1" for="rule-protocol">Protocol</label>
          <select
            id="rule-protocol"
            bind:value={formProtocol}
            class="w-full px-3 py-2 bg-nord-0 border border-nord-3 rounded text-nord-4 text-sm focus:border-frost-cyan focus:outline-none"
          >
            <option value="tcp">TCP</option>
            <option value="udp">UDP</option>
            <option value="both">Both</option>
          </select>
        </div>
        <div class="flex items-end">
          <button
            class="w-full px-4 py-2 text-sm bg-aurora-green hover:bg-aurora-green/80 text-nord-0 font-medium rounded transition-colors disabled:opacity-50"
            disabled={saving}
            onclick={addRule}
          >{saving ? "Saving..." : "Save"}</button>
        </div>
      </div>
    </div>
  {/if}

  <!-- Rules List -->
  {#if initialLoading}
    <p class="text-nord-3 text-sm">Loading...</p>
  {:else if rules.length > 0}
    <h2 class="text-sm font-semibold text-nord-5 mb-3 uppercase tracking-wider">Listener Rules</h2>
    <div class="bg-nord-1 rounded-lg overflow-hidden mb-6">
      <table class="w-full text-sm">
        <thead>
          <tr class="border-b border-nord-2 text-xs uppercase tracking-wider text-nord-3">
            <th class="text-left px-4 py-3 font-medium">#</th>
            <th class="text-left px-4 py-3 font-medium">Bind</th>
            <th class="text-left px-4 py-3 font-medium">Port</th>
            <th class="text-left px-4 py-3 font-medium">Protocol</th>
            <th class="text-right px-4 py-3 font-medium">Actions</th>
          </tr>
        </thead>
        <tbody>
          {#each rules as rule, i}
            <tr class="border-b border-nord-2/50 hover:bg-nord-2/30 transition-colors">
              <td class="px-4 py-3 text-nord-3 text-xs">{i + 1}</td>
              <td class="px-4 py-3 font-mono text-nord-4">{rule.bind ?? "0.0.0.0"}</td>
              <td class="px-4 py-3 font-mono text-frost-blue">{rule.port}</td>
              <td class="px-4 py-3 text-xs uppercase text-frost-teal">{rule.protocol ?? "tcp"}</td>
              <td class="px-4 py-3 text-right">
                <button
                  class="px-2 py-1 text-xs bg-aurora-red/20 hover:bg-aurora-red/40 text-aurora-red rounded transition-colors disabled:opacity-50"
                  disabled={saving}
                  onclick={() => removeRule(i)}
                >Delete</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {:else}
    <div class="bg-nord-1 rounded-lg p-8 text-center mb-6">
      <p class="text-nord-3 mb-2">No listener rules configured</p>
      <p class="text-nord-3/60 text-sm">Click "Add Rule" to define which ports Vane should listen on.</p>
    </div>
  {/if}

  <!-- Compiled Preview -->
  <h2 class="text-sm font-semibold text-nord-5 mb-3 uppercase tracking-wider">Compiled Listeners</h2>

  {#if compileError}
    <div class="p-3 rounded bg-nord-1 border border-aurora-red/40 text-aurora-red text-sm mb-4">
      {compileError}
    </div>
  {/if}

  {#if compiled.length > 0}
    <div class="text-xs text-nord-3 mb-2">{compiled.length} listener{compiled.length === 1 ? "" : "s"} total</div>
    <div class="bg-nord-1 rounded-lg overflow-hidden max-h-64 overflow-y-auto">
      <table class="w-full text-sm">
        <thead class="sticky top-0 bg-nord-1">
          <tr class="border-b border-nord-2 text-xs uppercase tracking-wider text-nord-3">
            <th class="text-left px-4 py-2 font-medium">Address</th>
            <th class="text-left px-4 py-2 font-medium">Port</th>
            <th class="text-left px-4 py-2 font-medium">Protocol</th>
          </tr>
        </thead>
        <tbody>
          {#each compiled as entry}
            <tr class="border-b border-nord-2/30 text-xs">
              <td class="px-4 py-1.5 font-mono text-nord-4">{entry.bind}</td>
              <td class="px-4 py-1.5 font-mono text-frost-blue">{entry.port}</td>
              <td class="px-4 py-1.5 uppercase text-frost-teal">{entry.protocol}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {:else if !compileError && rules.length === 0}
    <p class="text-nord-3 text-sm">No rules to compile.</p>
  {/if}
</div>
