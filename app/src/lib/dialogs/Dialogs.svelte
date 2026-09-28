<script lang="ts">
  import { dialogs } from "../stores/dialogs.svelte";
  import ConfirmDialog from "./ConfirmDialog.svelte";
  import CloudDialog from "./CloudDialog.svelte";
  import ConnectDialog from "./ConnectDialog.svelte";
  import DiffDialog from "./DiffDialog.svelte";
  import DestinationDialog from "./DestinationDialog.svelte";
  import HostKeyDialog from "./HostKeyDialog.svelte";
  import MultiRenameDialog from "./MultiRenameDialog.svelte";
  import OfferDialog from "./OfferDialog.svelte";
  import OnboardingDialog from "./OnboardingDialog.svelte";
  import PairDialog from "./PairDialog.svelte";
  import PromptDialog from "./PromptDialog.svelte";
  import SelectPatternDialog from "./SelectPatternDialog.svelte";
  import SendToDialog from "./SendToDialog.svelte";
  import SettingsDialog from "./SettingsDialog.svelte";
  import SignInDialog from "./SignInDialog.svelte";
  import TagsDialog from "./TagsDialog.svelte";

  const components = {
    confirm: ConfirmDialog,
    prompt: PromptDialog,
    connect: ConnectDialog,
    signIn: SignInDialog,
    hostKey: HostKeyDialog,
    settings: SettingsDialog,
    onboarding: OnboardingDialog,
    multiRename: MultiRenameDialog,
    selectPattern: SelectPatternDialog,
    sendTo: SendToDialog,
    offer: OfferDialog,
    diff: DiffDialog,
    pair: PairDialog,
    tags: TagsDialog,
    destination: DestinationDialog,
    cloud: CloudDialog,
  } as const;

  // When a dialog on top closes, hand focus back to the one underneath.
  let depth = 0;
  $effect(() => {
    const n = dialogs.stack.length;
    if (n < depth && n > 0) requestAnimationFrame(() => document.querySelectorAll<HTMLElement>(".layer .modal")[n - 1]?.focus());
    depth = n;
  });
</script>

{#each dialogs.stack as d, i (d)}
  {#if d.kind in components}
    {@const Dialog = components[d.kind as keyof typeof components] as any}
    <!-- Dialogs below the top one stay visible but can't be used until it closes. -->
    <div class="layer" inert={i < dialogs.stack.length - 1}>
      <Dialog {...d.props} />
    </div>
  {/if}
{/each}

<style>
  .layer {
    display: contents;
  }
</style>
