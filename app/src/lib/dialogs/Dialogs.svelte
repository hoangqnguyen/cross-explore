<script lang="ts">
  import { dialogs } from "../stores/dialogs.svelte";
  import ConfirmDialog from "./ConfirmDialog.svelte";
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
  } as const;
</script>

{#if dialogs.current && dialogs.current.kind in components}
  {@const Dialog = components[dialogs.current.kind as keyof typeof components] as any}
  {#key dialogs.current}
    <Dialog {...dialogs.current.props} />
  {/key}
{/if}
