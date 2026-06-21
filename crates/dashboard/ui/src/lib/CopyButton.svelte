<script>
  // Small copy-to-clipboard button. Shows a transient confirmation; degrades silently if the
  // Clipboard API is unavailable (e.g. non-secure context).
  let { text, label = "copy" } = $props();
  let done = $state(false);
  let failed = $state(false);
  async function copy() {
    try {
      await navigator.clipboard.writeText(String(text));
      done = true; failed = false;
      setTimeout(() => (done = false), 1200);
    } catch (_) {
      failed = true;
      setTimeout(() => (failed = false), 1200);
    }
  }
</script>

<button class="copy" onclick={copy} title="copy to clipboard">
  {done ? "✓ copied" : failed ? "✕ failed" : label}
</button>
