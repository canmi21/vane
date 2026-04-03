<script lang="ts">
  import { onMount } from "svelte";
  import { getSystemInfo } from "./lib/api";
  import Overview from "./pages/overview.svelte";
  import Ports from "./pages/ports.svelte";
  import Connections from "./pages/connections.svelte";

  type Page = "overview" | "ports" | "connections";

  const NAV: { id: Page; label: string }[] = [
    { id: "overview", label: "Overview" },
    { id: "ports", label: "Ports" },
    { id: "connections", label: "Connections" },
  ];

  function getPage(): Page {
    const hash = window.location.hash.slice(1);
    if (hash === "ports" || hash === "connections") return hash;
    return "overview";
  }

  let page: Page = $state(getPage());
  let version: string = $state("");

  function navigate(target: Page) {
    window.location.hash = target;
    page = target;
  }

  onMount(() => {
    window.addEventListener("hashchange", () => { page = getPage(); });
    getSystemInfo().then((info) => { version = info.version; }).catch(() => {});
  });
</script>

<div class="flex h-screen">
  <!-- Sidebar -->
  <nav class="w-56 bg-nord-1 flex flex-col shrink-0 border-r border-nord-2">
    <div class="px-5 py-5">
      <span class="text-lg font-bold tracking-wide text-frost-cyan">Vane</span>
      <span class="text-xs text-nord-3 ml-1.5">Console</span>
    </div>

    <ul class="flex-1 px-3 space-y-0.5">
      {#each NAV as item}
        <li>
          <button
            class="w-full text-left px-3 py-2 rounded text-sm transition-colors
              {page === item.id
                ? 'bg-nord-2 text-frost-cyan'
                : 'text-nord-4 hover:bg-nord-2/50 hover:text-nord-5'}"
            onclick={() => navigate(item.id)}
          >{item.label}</button>
        </li>
      {/each}
    </ul>

    {#if version}
      <div class="px-5 py-4 border-t border-nord-2">
        <span class="text-xs text-nord-3 font-mono">v{version}</span>
      </div>
    {/if}
  </nav>

  <!-- Main content -->
  <main class="flex-1 overflow-y-auto p-8">
    {#if page === "overview"}
      <Overview />
    {:else if page === "ports"}
      <Ports />
    {:else if page === "connections"}
      <Connections />
    {/if}
  </main>
</div>
