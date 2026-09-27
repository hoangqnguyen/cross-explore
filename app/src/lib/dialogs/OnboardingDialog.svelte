<script lang="ts">
  import DeviceIcon from "../components/DeviceIcon.svelte";
  import { devices } from "../stores/devices.svelte";
  import { dialogs } from "../stores/dialogs.svelte";
  import { settings } from "../stores/settings.svelte";
  import Modal from "./Modal.svelte";

  let step = $state(0);
  let s = settings.data;

  function finish() {
    s.onboarded = true;
    dialogs.close(null);
  }
</script>

<Modal title={step === 0 ? "Welcome to Cross Explore" : step === 1 ? "Your devices" : "All set"} width={560} onsubmit={() => (step < 2 ? step++ : finish())}>
  {#if step === 0}
    <p>How do you like to work? You can change this any time in Settings.</p>
    <div class="choices">
      <label class="choice" class:on={s.keymap !== "commander" && !s.dual}>
        <input type="radio" name="style" onchange={() => ((s.keymap = navigator.platform.includes("Mac") ? "finder" : "explorer"), (s.dual = false))} checked={s.keymap !== "commander" && !s.dual} />
        <div class="mock"><div class="bar"></div><div class="one"></div></div>
        <strong>{navigator.platform.includes("Mac") ? "Finder-like" : "Explorer-like"}</strong>
        <span>One pane with tabs and the keys you know. Finder and Explorer keys are both in Settings.</span>
      </label>
      <label class="choice" class:on={s.keymap === "commander" || s.dual}>
        <input type="radio" name="style" onchange={() => ((s.keymap = "commander"), (s.dual = true))} checked={s.keymap === "commander"} />
        <div class="mock"><div class="bar"></div><div class="two"><i></i><i></i></div></div>
        <strong>Commander-like</strong>
        <span>Two panes side by side. F5 copies, F6 moves, Tab switches.</span>
      </label>
    </div>
  {:else if step === 1}
    <p>Cross Explore finds shared folders on your network and your Tailscale tailnet. Nothing connects until you open it.</p>
    <div class="devs">
      {#each devices.nearby.slice(0, 6) as d (d.id)}
        <div class="dev"><DeviceIcon kind={d.kind} /><span>{d.name}</span><span class="muted">{d.services.map((x) => x.label).join(", ") || (d.tailnet ? "tailnet" : "")}</span></div>
      {:else}
        <p class="muted">{devices.scanning ? "Looking…" : "No devices found yet. They'll appear in the sidebar under Network."}</p>
      {/each}
    </div>
    <p class="muted">To browse this computer from your other devices, turn on sharing in Settings → Sharing & devices.</p>
  {:else}
    <p>Tips to get going:</p>
    <ul>
      <li><strong>Space</strong> previews the selected file with Quick Look.</li>
      <li><strong>Just type</strong> in a folder to filter it.</li>
      <li><strong>{navigator.platform.includes("Mac") ? "⌘P" : "Ctrl+P"}</strong> opens the command palette for everything else.</li>
      <li>Folders update <strong>live</strong> — no need to refresh.</li>
    </ul>
  {/if}
  {#snippet footer()}
    {#if step > 0}<button type="button" class="btn" onclick={() => step--}>Back</button>{/if}
    <button type="submit" class="btn primary">{step < 2 ? "Continue" : "Start exploring"}</button>
  {/snippet}
</Modal>

<style>
  .choices {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
  }
  .choice {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 14px;
    border-radius: var(--radius-lg);
    background: var(--layer-2);
    box-shadow: 0 0 0 1px var(--stroke-strong);
    font-size: 12px;
    color: var(--text-2);
  }
  .choice.on {
    box-shadow: 0 0 0 2px var(--accent);
  }
  .choice strong {
    color: var(--text);
    font-size: 13px;
  }
  .choice input {
    display: none;
  }
  .mock {
    height: 90px;
    border-radius: 6px;
    background: var(--hover);
    padding: 6px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .mock .bar {
    height: 10px;
    border-radius: 3px;
    background: var(--stroke-strong);
  }
  .mock .one,
  .mock .two i {
    flex: 1;
    border-radius: 4px;
    background: var(--layer);
  }
  .mock .two {
    flex: 1;
    display: flex;
    gap: 6px;
  }
  .devs {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-bottom: 10px;
  }
  .dev {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 32px;
    padding: 0 8px;
    border-radius: var(--radius);
    background: var(--layer-2);
    color: var(--accent);
  }
  .dev span:first-of-type {
    color: var(--text);
  }
  ul {
    margin: 0;
    padding-left: 18px;
    line-height: 1.9;
  }
</style>
