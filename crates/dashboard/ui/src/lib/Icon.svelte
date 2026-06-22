<script>
  // Inline SVG icon set (no icon dependency, no raw-HTML injection). Stroke-based, inherits color.
  let { name, size = 16, stroke = 2 } = $props();
</script>

<svg class="ico" width={size} height={size} viewBox="0 0 24 24" fill="none"
     stroke="currentColor" stroke-width={stroke} stroke-linecap="round" stroke-linejoin="round"
     aria-hidden="true">
  {#if name === "overview"}
    <rect x="3" y="3" width="7" height="9" rx="1.5" /><rect x="14" y="3" width="7" height="5" rx="1.5" />
    <rect x="14" y="12" width="7" height="9" rx="1.5" /><rect x="3" y="16" width="7" height="5" rx="1.5" />
  {:else if name === "upstreams"}
    <rect x="3" y="4" width="18" height="7" rx="2" /><rect x="3" y="13" width="18" height="7" rx="2" />
    <line x1="7" y1="7.5" x2="7.01" y2="7.5" /><line x1="7" y1="16.5" x2="7.01" y2="16.5" />
  {:else if name === "tools"}
    <path d="M14.7 6.3a4 4 0 0 0-5.4 5.4L4 17l3 3 5.3-5.3a4 4 0 0 0 5.4-5.4l-2.3 2.3-2-2 2.3-2.3z" />
  {:else if name === "calls"}
    <path d="M3 12h4l2 6 4-14 2 8h6" />
  {:else if name === "traces"}
    <circle cx="11" cy="11" r="7" /><line x1="21" y1="21" x2="16.65" y2="16.65" />
  {:else if name === "back"}
    <line x1="19" y1="12" x2="5" y2="12" /><polyline points="12 19 5 12 12 5" />
  {:else if name === "bolt"}
    <path d="M13 2 4 14h7l-1 8 9-12h-7l1-8z" />
  {:else if name === "server"}
    <rect x="4" y="4" width="16" height="6" rx="2" /><rect x="4" y="14" width="16" height="6" rx="2" />
  {:else if name === "wrench"}
    <path d="M14.7 6.3a4 4 0 0 0-5.4 5.4L4 17l3 3 5.3-5.3a4 4 0 0 0 5.4-5.4l-2.3 2.3-2-2 2.3-2.3z" />
  {:else if name === "clock"}
    <circle cx="12" cy="12" r="9" /><polyline points="12 7 12 12 15 14" />
  {:else if name === "layers"}
    <polygon points="12 2 2 7 12 12 22 7 12 2" /><polyline points="2 12 12 17 22 12" />
  {:else if name === "alert"}
    <path d="M12 9v4" /><path d="M12 17h.01" /><circle cx="12" cy="12" r="9" />
  {:else if name === "inbox"}
    <path d="M22 12h-6l-2 3h-4l-2-3H2" /><path d="M5.5 5h13L22 12v5a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2v-5z" />
  {:else if name === "gauge"}
    <path d="M12 13l4-4" /><circle cx="12" cy="13" r="9" /><path d="M3 13a9 9 0 0 1 18 0" />
  {:else if name === "refresh"}
    <polyline points="23 4 23 10 17 10" /><polyline points="1 20 1 14 7 14" />
    <path d="M3.5 9a9 9 0 0 1 14.9-3.4L23 10" /><path d="M1 14l4.6 4.4A9 9 0 0 0 20.5 15" />
  {:else if name === "info"}
    <circle cx="12" cy="12" r="9" /><line x1="12" y1="11" x2="12" y2="16" /><line x1="12" y1="8" x2="12.01" y2="8" />
  {/if}
</svg>
