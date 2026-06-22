<script>
  import { admin, adminPost } from "./admin.svelte.js";
  import { refreshNow } from "./refresh.svelte.js";
  // kind: "upstreams" | "tools"; disabled: current state (true => button enables, false => disables)
  let { kind, name, disabled } = $props();
  let busy = $state(false);
  let err = $state(false);
  async function toggle(e) {
    e.stopPropagation(); // don't trigger the row's navigation click
    busy = true;
    err = false;
    const action = disabled ? "enable" : "disable";
    try {
      const r = await adminPost(`/api/admin/${kind}/${encodeURIComponent(name)}/${action}`);
      if (!r.ok) {
        err = true; // 401 wrong/expired token, or 404 admin not configured / unknown name
        return;
      }
      refreshNow();
    } catch {
      err = true; // network error
    } finally {
      busy = false;
    }
  }
</script>

{#if admin.token}
  <button
    class="admbtn"
    class:err
    onclick={toggle}
    disabled={busy}
    title={err ? "failed — check admin token" : disabled ? "enable" : "disable"}
  >
    {disabled ? "enable" : "disable"}
  </button>
{/if}
