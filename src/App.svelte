<script lang="ts">
  import Splash from './splash/Splash.svelte'
  import Diagnostics from './diagnostics/Diagnostics.svelte'
  import Settings from './settings/Settings.svelte'
  import Skills from './skills/Skills.svelte'
  import Mcp from './mcp/Mcp.svelte'
  import Remote from './remote/Remote.svelte'

  let route = $state(window.location.hash)
  window.addEventListener('hashchange', () => {
    route = window.location.hash
  })
</script>

{#if route.startsWith('#/diagnostics')}
  <Diagnostics />
{:else if route.startsWith('#/settings')}
  <Settings />
{:else if route.startsWith('#/skills')}
  <Skills />
{:else if route.startsWith('#/mcp')}
  <Mcp />
{:else if route.startsWith('#/remote')}
  <Remote />
{:else}
  <Splash />
{/if}
