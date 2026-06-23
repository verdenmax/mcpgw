<script>
  // Raw TOML text editor. `content` is two-way bound to the parent.
  let { content = $bindable("") } = $props();
</script>

<textarea class="cfg-edit" bind:value={content} spellcheck="false" aria-label="config TOML"></textarea>
