// Nearby devices (discovery), open server connections and peer mode state.
import { connections as loadConnections, devices as loadDevices, errorText, peerStatus, refreshDiscovery, type Device, type IncomingOffer, type PeerStatus } from "../api";
import { toasts } from "../toasts.svelte";

class Devices {
  list = $state.raw<Device[]>([]);
  connected = $state.raw<string[]>([]);
  peer = $state.raw<PeerStatus | null>(null);
  offers = $state.raw<IncomingOffer[]>([]);
  scanning = $state(false);

  /** Devices worth showing: everything except this machine. */
  nearby = $derived(this.list.filter((d) => !d.tailnet?.isSelf).sort((a, b) => Number(!!b.tailnet?.online || !b.tailnet) - Number(!!a.tailnet?.online || !a.tailnet) || a.name.localeCompare(b.name)));

  async init() {
    try {
      [this.list, this.connected, this.peer] = await Promise.all([loadDevices(), loadConnections(), peerStatus()]);
    } catch {
      /* features not available in this build */
    }
  }

  async refreshConnections() {
    try {
      this.connected = await loadConnections();
    } catch {
      /* ignore */
    }
  }

  async scan() {
    this.scanning = true;
    try {
      await refreshDiscovery();
    } catch (e) {
      toasts.show(errorText(e), "error");
    }
    setTimeout(() => (this.scanning = false), 2500);
  }

  addOffer(o: IncomingOffer) {
    this.offers = [...this.offers, o];
  }

  dropOffer(id: string) {
    this.offers = this.offers.filter((o) => o.id !== id);
  }
}

export const devices = new Devices();
