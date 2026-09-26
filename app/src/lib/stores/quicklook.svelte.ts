// Quick Look: a floating preview of the focused item (Space to toggle).
class QuickLook {
  open = $state(false);

  toggle() {
    this.open = !this.open;
  }

  close() {
    this.open = false;
  }
}

export const quicklook = new QuickLook();
