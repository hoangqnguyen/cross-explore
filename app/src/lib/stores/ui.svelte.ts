// Layout mode: phones (and very narrow windows) get the touch layout.
class Ui {
  width = $state(typeof window === "undefined" ? 1200 : window.innerWidth);
  platform = $state("macos");
  drawerOpen = $state(false);
  terminalOpen = $state(false);
  /** Touch selection mode: taps toggle selection instead of opening. */
  selecting = $state(false);

  phone = $derived(this.platform === "ios" || this.platform === "android");
  mobile = $derived(this.phone || this.width < 640);

  constructor() {
    if (typeof window !== "undefined") window.addEventListener("resize", () => (this.width = window.innerWidth));
  }
}

export const ui = new Ui();
