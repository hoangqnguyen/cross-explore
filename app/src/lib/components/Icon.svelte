<script lang="ts" module>
  // 20×20 stroke icons in the spirit of Fluent System Icons.
  const paths = {
    back: "M11.5 4.5 6 10l5.5 5.5",
    forward: "M8.5 4.5 14 10l-5.5 5.5",
    up: "M10 15.5v-11M5 9l5-5 5 5",
    reload: "M15.5 10a5.5 5.5 0 1 1-1.6-3.9M15.5 3.5v3h-3",
    chevronRight: "M8 5.5 12.5 10 8 14.5",
    chevronDown: "M5.5 8 10 12.5 14.5 8",
    plus: "M10 4.5v11M4.5 10h11",
    close: "M5.5 5.5l9 9M14.5 5.5l-9 9",
    newFolder: "M2.5 6.5v7a2 2 0 0 0 2 2h5M2.5 6.5a2 2 0 0 1 2-2h3l1.5 1.5h6a2 2 0 0 1 2 2v2M14.5 12v5M12 14.5h5",
    rename: "M12.8 3.7a1.6 1.6 0 0 1 2.3 2.3L7 14.1l-3.2.9.9-3.2zM11.5 5l2.3 2.3",
    trash: "M3.5 5.5h13M8 5.5V4a1 1 0 0 1 1-1h2a1 1 0 0 1 1 1v1.5M5 5.5l.8 10a1.5 1.5 0 0 0 1.5 1.4h5.4a1.5 1.5 0 0 0 1.5-1.4l.8-10M8.5 8.5v5M11.5 8.5v5",
    sort: "M6 4v12M3 13l3 3 3-3M14 16V4M11 7l3-3 3 3",
    view: "M3.5 5h13M3.5 10h13M3.5 15h13",
    more: "M5 10h.01M10 10h.01M15 10h.01",
    search: "M8.5 14a5.5 5.5 0 1 0 0-11 5.5 5.5 0 0 0 0 11zM12.5 12.5l4 4",
    copy: "M7 6.5V4.5a1.5 1.5 0 0 1 1.5-1.5h7A1.5 1.5 0 0 1 17 4.5v7a1.5 1.5 0 0 1-1.5 1.5h-2M4.5 7h7A1.5 1.5 0 0 1 13 8.5v7a1.5 1.5 0 0 1-1.5 1.5h-7A1.5 1.5 0 0 1 3 15.5v-7A1.5 1.5 0 0 1 4.5 7z",
    open: "M11 3.5h5.5V9M16.5 3.5 9.5 10.5M14 12v3a1.5 1.5 0 0 1-1.5 1.5h-7A1.5 1.5 0 0 1 4 15V8a1.5 1.5 0 0 1 1.5-1.5h3",
    check: "M4.5 10.5 8 14l7.5-8",
    eye: "M2.5 10s2.7-5 7.5-5 7.5 5 7.5 5-2.7 5-7.5 5-7.5-5-7.5-5zM10 12a2 2 0 1 0 0-4 2 2 0 0 0 0 4z",
    rows: "M3 5.5h14M3 9h14M3 12.5h14M3 16h14",
    home: "M3.5 9 10 3.5 16.5 9M5 7.8V16h3.5v-4.5h3V16H15V7.8",
    desktop: "M3 4.5h14v9H3zM7 16.5h6M10 13.5v3",
    documents: "M5.5 2.5h6l3.5 3.5v11.5h-9.5zM11.5 2.5V6H15M8 10h5M8 13h5",
    downloads: "M10 3v10M6 9.5l4 4 4-4M4 16.5h12",
    pictures: "M3 4.5h14v11H3zM3 13l4-4 3 3 2-2 5 5M13 8.5h.01",
    music: "M8 15V5l8-1.5v10M8 15a2 2 0 1 1-4 0 2 2 0 0 1 4 0zM16 13.5a2 2 0 1 1-4 0 2 2 0 0 1 4 0z",
    videos: "M3 5h10v10H3zM13 8.5l4-2.5v8l-4-2.5",
    drive: "M3 11.5h14v4H3zM4 11.5l2-6.5h8l2 6.5M13.5 13.5h.01",
    external: "M5 3.5h10v13H5zM8 13.5h4M10 6.5v3",
    folder: "M2.5 5.5a2 2 0 0 1 2-2h3l1.5 1.5h6a2 2 0 0 1 2 2v7.5a2 2 0 0 1-2 2h-10.5a2 2 0 0 1-2-2z",
    minimize: "M5 10h10",
    maximize: "M5.5 5.5h9v9h-9z",
    winClose: "M5.5 5.5l9 9M14.5 5.5l-9 9",
  } as const;
  export type IconName = keyof typeof paths;
</script>

<script lang="ts">
  let { name, size = 16, stroke = 1.35 }: { name: IconName; size?: number; stroke?: number } = $props();
</script>

<svg width={size} height={size} viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width={stroke} stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
  <path d={paths[name]} />
</svg>

<style>
  svg {
    flex: none;
    display: block;
  }
</style>
