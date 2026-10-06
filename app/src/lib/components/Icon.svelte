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
    code: "M7.5 6 3 10l4.5 4M12.5 6 17 10l-4.5 4M11.2 4.5 8.8 15.5",
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
    cut: "M6 17a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5zM14 17a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5zM7.8 12.4 14.5 3M12.2 12.4 5.5 3",
    paste: "M7 4.5H5.5A1.5 1.5 0 0 0 4 6v10a1.5 1.5 0 0 0 1.5 1.5h9A1.5 1.5 0 0 0 16 16V6a1.5 1.5 0 0 0-1.5-1.5H13M7.5 3h5v3h-5z",
    undo: "M7 5 3.5 8.5 7 12M3.5 8.5H12a4.5 4.5 0 0 1 0 9H9",
    move: "M3 10h12M11 6l4 4-4 4M17 4v12",
    link: "M8.5 11.5a3 3 0 0 0 4.2 0l2.6-2.6a3 3 0 0 0-4.2-4.2l-.8.8M11.5 8.5a3 3 0 0 0-4.2 0l-2.6 2.6a3 3 0 0 0 4.2 4.2l.8-.8",
    terminal: "M3 4.5h14v11H3zM6 8l2.5 2L6 12M10 12.5h4",
    sigma: "M15 4.5H5l5 5.5-5 5.5h10",
    archive: "M3.5 4h13v3.5h-13zM4.5 7.5v8a1 1 0 0 0 1 1h9a1 1 0 0 0 1-1v-8M8 10.5h4",
    tag: "M3 3.5h6.5l7.5 7.5-6 6-7.5-7.5zM6.5 7h.01",
    send: "M17 3 3 9l5.5 2.5L11 17zM8.5 11.5 17 3",
    diff: "M6 3v14M14 3v14M3 7h6M11 13h6M3 13h6M11 7h6",
    sync: "M4 8a6 6 0 0 1 10.5-2.5L16 7M16 3.5V7h-3.5M16 12a6 6 0 0 1-10.5 2.5L4 13M4 16.5V13h3.5",
    star: "M10 3l2.2 4.6 5 .7-3.6 3.5.9 5-4.5-2.4-4.5 2.4.9-5L2.8 8.3l5-.7z",
    cloud: "M6 15.5h8.5a3.5 3.5 0 0 0 .4-7 5 5 0 0 0-9.6 1.2A3 3 0 0 0 6 15.5z",
    server: "M3.5 3.5h13v5h-13zM3.5 11.5h13v5h-13zM6.5 6h.01M6.5 14h.01",
    radar: "M10 10l5-5M10 17a7 7 0 1 1 7-7M10 13.5a3.5 3.5 0 1 1 3.5-3.5",
    transfer: "M4 7h11M12 4l3 3-3 3M16 13H5M8 10l-3 3 3 3",
    command: "M7.5 5.5v9M12.5 5.5v9M5.5 7.5h9M5.5 12.5h9M7.5 5.5a2 2 0 1 0-2 2M12.5 5.5a2 2 0 1 1 2 2M7.5 14.5a2 2 0 1 1-2-2M12.5 14.5a2 2 0 1 0 2-2",
    settings: "M10 12.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5zM16 10l1.5-.9-1.5-2.6-1.7.4a5.5 5.5 0 0 0-1.4-.8L12.5 4.4h-3l-.4 1.7a5.5 5.5 0 0 0-1.4.8L6 6.5 4.5 9.1 6 10l-1.5.9L6 13.5l1.7-.4a5.5 5.5 0 0 0 1.4.8l.4 1.7h3l.4-1.7a5.5 5.5 0 0 0 1.4-.8l1.7.4 1.5-2.6z",
    columns: "M3 4h14v12H3zM8 4v12M12.5 4v12",
    grid: "M3.5 3.5h5v5h-5zM11.5 3.5h5v5h-5zM3.5 11.5h5v5h-5zM11.5 11.5h5v5h-5z",
    gallery: "M3 3.5h14v9H3zM3 15.5h3M8.5 15.5h3M14 15.5h3",
    sidebarRight: "M3 4h14v12H3zM12.5 4v12",
    panelBottom: "M3 4h14v12H3zM3 11.5h14",
    pause: "M7 5v10M13 5v10",
    play: "M6.5 4.5v11l9-5.5z",
    phone: "M6.5 2.5h7v15h-7zM9.5 15h1",
    laptop: "M4.5 5h11v7.5h-11zM2.5 15.5h15",
    nas: "M5 3h10v14H5zM7.5 6h5M7.5 9h5M7.5 14h.01",
    pc: "M3 4h14v9H3zM7 16h6M10 13v3",
    info: "M10 17a7 7 0 1 0 0-14 7 7 0 0 0 0 14zM10 9v4.5M10 6.5h.01",
    warning: "M10 3 2.5 16.5h15zM10 8v4M10 14.5h.01",
    lock: "M5.5 9h9v7.5h-9zM7.5 9V6.5a2.5 2.5 0 0 1 5 0V9",
    user: "M10 10a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7zM3.5 17.5a6.5 6.5 0 0 1 13 0",
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
