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
    // Load independently so one failing call doesn't hide the others.
    await Promise.all([
      loadDevices().then((v) => (this.list = v)).catch(() => {}),
      loadConnections().then((v) => (this.connected = v)).catch(() => {}),
      this.refreshPeer(),
    ]);
  }

  /** Peer status; the service starts in the background, so retry while it comes up. */
  async refreshPeer(tries = 8) {
    for (let i = 0; i < tries && !this.peer; i++) {
      try {
        this.peer = await peerStatus();
        return;
      } catch {
        await new Promise((r) => setTimeout(r, 750));
      }
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

/** The best way to browse a device: peer, then SMB, SFTP, WebDAV, FTP. */
export function deviceTarget(d: Device): string | null {
  const order = ["peer", "smb", "sftp", "davs", "dav", "ftps", "ftp"];
  return [...d.services].sort((a, b) => order.indexOf(a.scheme) - order.indexOf(b.scheme))[0]?.uri ?? null;
}
