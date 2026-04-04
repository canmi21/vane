<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { getConfig, updateConfig, compileListeners } from "../lib/api";
  import type { ListenerRule, Protocol, CompiledListener } from "../types/bindings";
  import { z } from "zod/v4";

  // -- Validation -----------------------------------------------------------

  const portSpec = z.string().check(
    z.refine((v) => {
      const m = v.match(/^(\d+)(?:-(\d+))?$/);
      if (!m) return false;
      const s = Number(m[1]), e = m[2] ? Number(m[2]) : s;
      return s >= 1 && s <= 65535 && e >= 1 && e <= 65535 && s <= e;
    }, "Invalid port or range")
  );

  const ipAddr = z.string().check(
    z.refine((v) => {
      if (v === "0.0.0.0" || v === "::" || v === "::1") return true;
      const ipv4 = /^(\d{1,3}\.){3}\d{1,3}$/.test(v) && v.split(".").every((o) => Number(o) <= 255);
      return ipv4 || v.includes(":");
    }, "Invalid IP address")
  );

  function validateField(schema: z.ZodType, value: string): string | null {
    const r = schema.safeParse(value);
    return r.success ? null : r.error.issues[0]?.message ?? "Invalid";
  }

  // -- State ----------------------------------------------------------------

  let rules: ListenerRule[] = $state([]);
  let compiled: CompiledListener[] = $state([]);
  let compileError: string | null = $state(null);
  let error: string | null = $state(null);
  let saving = $state(false);
  let initialLoading = $state(true);

  // Editing state: index of the row being edited, or -1 for the new row at bottom
  let editingIndex: number | null = $state(null);
  let editBind = $state("0.0.0.0");
  let editPort = $state("");
  let editProtocol: Protocol = $state("any");

  // Per-field errors for the active edit row
  let errBind: string | null = $state(null);
  let errPort: string | null = $state(null);

  $effect(() => { errBind = editBind ? validateField(ipAddr, editBind) : null; });
  $effect(() => { errPort = editPort ? validateField(portSpec, editPort) : null; });

  let editValid = $derived(!errBind && !errPort && editPort.length > 0);

  // -- Data loading ---------------------------------------------------------

  async function load() {
    try {
      const configResult = await getConfig();
      const config = configResult.config as unknown as Record<string, unknown>;
      rules = (config.rules ?? []) as ListenerRule[];
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

  // -- Persist helpers ------------------------------------------------------

  async function applyRules(newRules: ListenerRule[]) {
    saving = true;
    error = null;
    try {
      // Compile rules into concrete listeners for the engine
      const compileResult = await compileListeners({ listeners: newRules });
      if (!compileResult.ok) {
        error = compileResult.error ?? "Compilation failed";
        return;
      }

      const configResult = await getConfig();
      const config = configResult.config as unknown as Record<string, unknown>;
      config.listeners = compileResult.listeners; // engine sees compiled entries
      config.rules = newRules; // preserve original rules for UI
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

  // -- Row actions ----------------------------------------------------------

  function startAdd() {
    editingIndex = -1;
    editBind = "0.0.0.0";
    editPort = "";
    editProtocol = "any";
  }

  function startEdit(i: number) {
    editingIndex = i;
    const r = rules[i];
    editBind = r.bind ?? "0.0.0.0";
    editPort = r.port;
    editProtocol = r.protocol ?? "any";
  }

  function cancelEdit() {
    editingIndex = null;
  }

  async function confirmEdit() {
    if (!editValid) return;
    const rule: ListenerRule = { bind: editBind, port: editPort, protocol: editProtocol };
    const newRules = [...rules];
    if (editingIndex === -1) {
      newRules.push(rule);
    } else if (editingIndex !== null) {
      newRules[editingIndex] = rule;
    }
    editingIndex = null;
    await applyRules(newRules);
  }

  async function removeRule(i: number) {
    const newRules = rules.filter((_, idx) => idx !== i);
    if (editingIndex === i) editingIndex = null;
    await applyRules(newRules);
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && editValid) confirmEdit();
    if (e.key === "Escape") cancelEdit();
  }

  let timer: ReturnType<typeof setInterval>;
  onMount(() => { load(); timer = setInterval(load, 5000); });
  onDestroy(() => { clearInterval(timer); });

  // Shared input classes
  const inputCls = "w-full px-2 py-1 bg-nord-0 border rounded text-nord-4 font-mono text-sm focus:outline-none";
  const inputOk = "border-nord-3 focus:border-frost-cyan";
  const inputErr = "border-aurora-red focus:border-aurora-red";
</script>

<div>
  <div class="flex items-center justify-between mb-6">
    <h1 class="text-2xl font-semibold text-nord-6">Listener Rules</h1>
    <button
      class="px-3 py-1.5 text-sm bg-frost-deep hover:bg-frost-blue text-nord-6 rounded transition-colors"
      onclick={startAdd}
      disabled={editingIndex !== null}
    >+ Add</button>
  </div>

  {#if error}
    <div class="p-3 rounded bg-nord-1 border border-aurora-red/40 text-aurora-red text-sm mb-4">{error}</div>
  {/if}

  <!-- Rules table (always visible) -->
  {#if initialLoading}
    <p class="text-nord-3 text-sm">Loading...</p>
  {:else}
    <div class="bg-nord-1 rounded-lg overflow-hidden mb-6">
      <table class="w-full text-sm">
        <thead>
          <tr class="border-b border-nord-2 text-xs tracking-wide text-nord-4/70">
            <th class="text-left px-4 py-3 font-medium w-12">#</th>
            <th class="text-left px-4 py-3 font-medium">Bind</th>
            <th class="text-left px-4 py-3 font-medium">Port</th>
            <th class="text-left px-4 py-3 font-medium w-28">Protocol</th>
            <th class="text-right px-4 py-3 font-medium w-24">Actions</th>
          </tr>
        </thead>
        <tbody>
          {#each rules as rule, i}
            {#if editingIndex === i}
              <!-- Editing existing row -->
              <tr class="border-b border-frost-cyan/30 bg-nord-2/20">
                <td class="px-4 py-2 text-nord-3 text-xs">{i + 1}</td>
                <td class="px-4 py-2">
                  <input type="text" bind:value={editBind} onkeydown={handleKeydown}
                    class="{inputCls} {errBind ? inputErr : inputOk}" />
                  {#if errBind}<div class="text-aurora-red text-[10px] mt-0.5">{errBind}</div>{/if}
                </td>
                <td class="px-4 py-2">
                  <input type="text" bind:value={editPort} onkeydown={handleKeydown} placeholder="8080 or 8000-8100"
                    class="{inputCls} {errPort ? inputErr : inputOk}" />
                  {#if errPort}<div class="text-aurora-red text-[10px] mt-0.5">{errPort}</div>{/if}
                </td>
                <td class="px-4 py-2">
                  <select bind:value={editProtocol}
                    class="w-full px-2 py-1 bg-nord-0 border border-nord-3 rounded text-nord-4 text-sm focus:border-frost-cyan focus:outline-none">
                    <option value="tcp">TCP</option>
                    <option value="udp">UDP</option>
                    <option value="any">Any</option>
                  </select>
                </td>
                <td class="px-4 py-2 text-right space-x-1">
                  <button class="px-2 py-1 text-xs bg-aurora-green/20 hover:bg-aurora-green/40 text-aurora-green rounded disabled:opacity-30"
                    disabled={!editValid || saving} onclick={confirmEdit}>OK</button>
                  <button class="px-2 py-1 text-xs bg-nord-3/30 hover:bg-nord-3/50 text-nord-4 rounded"
                    onclick={cancelEdit}>X</button>
                </td>
              </tr>
            {:else}
              <!-- Display row -->
              <tr class="border-b border-nord-2/50 hover:bg-nord-2/30 transition-colors cursor-pointer group"
                ondblclick={() => startEdit(i)}>
                <td class="px-4 py-3 text-nord-3 text-xs">{i + 1}</td>
                <td class="px-4 py-3 font-mono text-nord-4">{rule.bind ?? "0.0.0.0"}</td>
                <td class="px-4 py-3 font-mono text-frost-blue">{rule.port}</td>
                <td class="px-4 py-3 text-xs uppercase text-frost-teal">{rule.protocol ?? "any"}</td>
                <td class="px-4 py-3 text-right space-x-1 opacity-0 group-hover:opacity-100 transition-opacity">
                  <button class="px-2 py-1 text-xs bg-nord-2 hover:bg-nord-3 text-nord-4 rounded"
                    onclick={() => startEdit(i)}>Edit</button>
                  <button class="px-2 py-1 text-xs bg-aurora-red/20 hover:bg-aurora-red/40 text-aurora-red rounded disabled:opacity-50"
                    disabled={saving} onclick={() => removeRule(i)}>Del</button>
                </td>
              </tr>
            {/if}
          {/each}

          <!-- New row (when adding) -->
          {#if editingIndex === -1}
            <tr class="border-b border-frost-cyan/30 bg-nord-2/20">
              <td class="px-4 py-2 text-nord-3 text-xs">+</td>
              <td class="px-4 py-2">
                <input type="text" bind:value={editBind} onkeydown={handleKeydown}
                  class="{inputCls} {errBind ? inputErr : inputOk}" />
                {#if errBind}<div class="text-aurora-red text-[10px] mt-0.5">{errBind}</div>{/if}
              </td>
              <td class="px-4 py-2">
                <input type="text" bind:value={editPort} onkeydown={handleKeydown} placeholder="8080 or 8000-8100"
                  class="{inputCls} {errPort ? inputErr : inputOk}" />
                {#if errPort}<div class="text-aurora-red text-[10px] mt-0.5">{errPort}</div>{/if}
              </td>
              <td class="px-4 py-2">
                <select bind:value={editProtocol}
                  class="w-full px-2 py-1 bg-nord-0 border border-nord-3 rounded text-nord-4 text-sm focus:border-frost-cyan focus:outline-none">
                  <option value="tcp">TCP</option>
                  <option value="udp">UDP</option>
                  <option value="any">Any</option>
                </select>
              </td>
              <td class="px-4 py-2 text-right space-x-1">
                <button class="px-2 py-1 text-xs bg-aurora-green/20 hover:bg-aurora-green/40 text-aurora-green rounded disabled:opacity-30"
                  disabled={!editValid || saving} onclick={confirmEdit}>OK</button>
                <button class="px-2 py-1 text-xs bg-nord-3/30 hover:bg-nord-3/50 text-nord-4 rounded"
                  onclick={cancelEdit}>X</button>
              </td>
            </tr>
          {/if}
        </tbody>
      </table>

      {#if rules.length === 0 && editingIndex !== -1}
        <div class="px-4 py-6 text-center text-nord-3 text-sm">
          No rules. Click "+ Add" to get started.
        </div>
      {/if}
    </div>
  {/if}

  <!-- Compile preview -->
  <h2 class="text-sm font-semibold text-nord-4 mb-3">Compiled Listeners</h2>

  {#if compileError}
    <div class="p-3 rounded bg-nord-1 border border-aurora-red/40 text-aurora-red text-sm mb-4">{compileError}</div>
  {/if}

  {#if compiled.length > 0}
    <div class="text-xs text-nord-3 mb-2">{compiled.length} listener{compiled.length === 1 ? "" : "s"}</div>
    <div class="bg-nord-1 rounded-lg overflow-hidden max-h-64 overflow-y-auto">
      <table class="w-full text-sm">
        <thead class="sticky top-0 bg-nord-1">
          <tr class="border-b border-nord-2 text-xs tracking-wide text-nord-4/70">
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
