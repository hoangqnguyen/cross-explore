// Finder's tag colors; other tag names get a neutral dot.
export const TAG_COLORS: Record<string, string> = {
  Red: "#ff3b30",
  Orange: "#ff9500",
  Yellow: "#ffcc00",
  Green: "#34c759",
  Blue: "#007aff",
  Purple: "#af52de",
  Gray: "#8e8e93",
};

export const tagColor = (t: string) => TAG_COLORS[t] ?? "var(--text-3)";
