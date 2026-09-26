<script lang="ts">
  import type { DeviceKind } from "../api";
  import Icon, { type IconName } from "./Icon.svelte";

  let { kind, size = 18 }: { kind: DeviceKind; size?: number } = $props();
  const map: Record<DeviceKind, IconName> = { mac: "laptop", pc: "pc", linux: "server", nas: "nas", phone: "phone", tablet: "phone", router: "radar", printer: "server", unknown: "server" };
</script>

<Icon name={map[kind] ?? "server"} {size} />
