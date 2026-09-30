<script lang="ts">
  // An iOS/macOS-style switch: a real checkbox underneath (keyboard, a11y,
  // forms all work normally), just drawn as a pill instead of a native box.
  let { checked = $bindable(false), disabled = false, onchange, label }: { checked?: boolean; disabled?: boolean; onchange?: (checked: boolean) => void; label?: string } = $props();
</script>

<!-- svelte-ignore a11y_label_has_associated_control -->
<label class="toggle" class:disabled aria-label={label}>
  <input
    type="checkbox"
    bind:checked
    {disabled}
    onchange={(e) => onchange?.((e.currentTarget as HTMLInputElement).checked)}
  />
  <span class="track"><span class="thumb"></span></span>
</label>

<style>
  .toggle {
    display: inline-flex;
    flex: none;
    cursor: pointer;
  }
  .toggle.disabled {
    cursor: default;
    opacity: 0.5;
  }
  input {
    position: absolute;
    width: 1px;
    height: 1px;
    opacity: 0;
    pointer-events: none;
  }
  .track {
    position: relative;
    width: 36px;
    height: 21px;
    border-radius: 999px;
    background: var(--stroke-strong);
    transition: background 0.15s var(--ease);
  }
  input:checked + .track {
    background: var(--accent);
  }
  input:focus-visible + .track {
    box-shadow: 0 0 0 2px var(--layer-2), 0 0 0 4px var(--accent);
  }
  .thumb {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 17px;
    height: 17px;
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.3);
    transition: transform 0.15s var(--ease);
  }
  input:checked + .track .thumb {
    transform: translateX(15px);
  }
</style>
