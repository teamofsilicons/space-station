// One stroke icon set on a 16-unit grid, drawn in currentColor, and the station's mark.

const PATHS: Record<string, string> = {
  blank: "",
  home: "M2.5 7.2 8 2.8l5.5 4.4M4 6.2v7.3h3V10h2v3.5h3V6.2",
  table: "M2.5 3h11v10h-11zM2.5 6.3h11M2.5 9.7h11M6.2 6.3V13",
  window: "M2 3h12v10H2zM2 5.8h12M4.2 4.4h.01M5.9 4.4h.01M7.6 4.4h.01",
  bell: "M4 11.2V7.3a4 4 0 0 1 8 0v3.9l1.2 1.3H2.8zM6.6 14.3h2.8",
  settings: "M2.5 4.5h7M12.5 4.5h1M2.5 11.5h1M6.5 11.5h7M11 3v3M5 10v3",
  docs: "M8 4.3C6.8 3.2 4.9 3 2.5 3.2v9.3c2.4-.2 4.3 0 5.5 1.1 1.2-1.1 3.1-1.3 5.5-1.1V3.2C11.1 3 9.2 3.2 8 4.3zM8 4.3v9.3",
  search: "M7 11.8a4.8 4.8 0 1 0 0-9.6 4.8 4.8 0 0 0 0 9.6zM10.5 10.5l3.3 3.3",
  plus: "M8 3v10M3 8h10",
  x: "M4.2 4.2l7.6 7.6M11.8 4.2l-7.6 7.6",
  back: "M13 8H3.5M7.2 4.2 3.4 8l3.8 3.8",
  forward: "M3 8h9.5M8.8 4.2 12.6 8l-3.8 3.8",
  reload: "M13.2 8.3a5.2 5.2 0 1 1-1.6-4.1M13.2 2.6v2.9h-2.9",
  split: "M2 3h12v10H2zM8 3v10",
  more: "M3.5 8h.01M8 8h.01M12.5 8h.01",
  pin: "M5.8 2.5h4.4M7 2.5v3.8L4.8 9h6.4L9 6.3V2.5M8 9v4.8",
  chevron: "M4.5 6.2 8 9.7l3.5-3.5",
  code: "M5.8 4.2 2 8l3.8 3.8M10.2 4.2 14 8l-3.8 3.8",
  key: "M10.2 9.8A3.6 3.6 0 1 0 6.8 7.3L2.5 11.6v1.9h2.2V12h1.5v-1.5h1.5l.9-.9a3.6 3.6 0 0 0 1.6.2zM11.2 5.3h.01",
  panel: "M2 3h12v10H2zM6 3v10",
  bug: "M5.6 5.4a2.4 2.4 0 0 1 4.8 0M4.6 6.4h6.8v3.2a3.4 3.4 0 0 1-6.8 0zM8 8.2v4.8M2.2 8.7h2.4M11.4 8.7h2.4M2.8 5.2l1.8 1.3M13.2 5.2l-1.8 1.3M2.8 13l1.9-1.6M13.2 13l-1.9-1.6",
  sun: "M8 10.8a2.8 2.8 0 1 0 0-5.6 2.8 2.8 0 0 0 0 5.6zM8 1.5v1.3M8 13.2v1.3M1.5 8h1.3M13.2 8h1.3M3.4 3.4l.9.9M11.7 11.7l.9.9M3.4 12.6l.9-.9M11.7 4.3l.9-.9",
  moon: "M13.2 9.6A5.6 5.6 0 0 1 6.4 2.8a5.6 5.6 0 1 0 6.8 6.8z",
  external: "M9.2 2.8h4v4M13.2 2.8 7.6 8.4M11.4 9.6v3.6H2.8V4.6h3.6",
  copy: "M5.6 5.6h7.6v7.6H5.6zM3.2 10.4V2.8h7.6",
  check: "M3 8.6l3.1 3 6.9-7",
  user: "M8 7.8a2.7 2.7 0 1 0 0-5.4 2.7 2.7 0 0 0 0 5.4zM2.6 14a5.4 5.4 0 0 1 10.8 0",
  logout: "M6.2 2.8H2.8v10.4h3.4M10.4 4.8 13.6 8l-3.2 3.2M13.6 8H6.2",
  keyboard: "M1.8 4h12.4v8H1.8zM4.2 6.5h.01M6.7 6.5h.01M9.3 6.5h.01M11.8 6.5h.01M5 9.5h6",
  archive: "M2 3h12v3H2zM3 6v7h10V6M6.5 8.8h3",
  trash: "M2.8 4.3h10.4M6.2 4.3V2.8h3.6v1.5M4.3 4.3l.6 9h6.2l.6-9",
  link: "M6.9 9.1a2.9 2.9 0 0 0 4.1 0l2-2a2.9 2.9 0 0 0-4.1-4.1l-.9.9M9.1 6.9a2.9 2.9 0 0 0-4.1 0l-2 2a2.9 2.9 0 0 0 4.1 4.1l.9-.9",
  pencil: "M10.6 2.9 13.1 5.4 5.8 12.7 2.8 13.2l.5-3z",
  play: "M5 3.3v9.4L12.6 8z",
  pause: "M5.5 3.5v9M10.5 3.5v9",
  pulse: "M1.8 8.2h3l1.6-4 3.2 8 1.6-4h3",
  clock: "M8 14a6 6 0 1 0 0-12 6 6 0 0 0 0 12zM8 4.8V8l2.2 1.4",
  flask: "M6.2 2.5h3.6M6.8 2.5v4L3.2 12.6a.9.9 0 0 0 .8 1.4h8a.9.9 0 0 0 .8-1.4L9.2 6.5v-4M4.8 10h6.4",
  list: "M5.5 4h8M5.5 8h8M5.5 12h8M2.5 4h.01M2.5 8h.01M2.5 12h.01",
  command: "M6 6V4.5A1.5 1.5 0 1 0 4.5 6H6zm0 0h4m-4 0v4m4-4V4.5A1.5 1.5 0 1 1 11.5 6H10zm0 0v4m0 0h1.5A1.5 1.5 0 1 1 10 11.5V10zm0 0H6m0 0v1.5A1.5 1.5 0 1 1 4.5 10H6z",
  duplicate: "M5.6 5.6h7.6v7.6H5.6zM3.2 10.4V2.8h7.6M9.4 7.6v3.6M7.6 9.4h3.6",
};

export function Icon(p: { name: string; size?: number; class?: string }) {
  return (
    <svg
      class={`icon ${p.class || ""}`}
      viewBox="0 0 16 16"
      width={p.size || 16}
      height={p.size || 16}
      fill="none"
      stroke="currentColor"
      stroke-width={p.name === "more" ? 2.4 : 1.4}
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <path d={PATHS[p.name] ?? ""} />
    </svg>
  );
}

/** The station's mark: eight squares and the one turned 45°. */
export function Mark(p: { size?: number }) {
  return (
    <svg class="mark" viewBox="0 0 355 355" width={p.size || 18} height={p.size || 18} fill="currentColor" aria-hidden="true">
      <rect y="131.865" width="91.286" height="91.287" />
      <rect x="131.86" width="91.286" height="91.287" />
      <rect x="263.718" y="131.865" width="91.286" height="91.287" />
      <rect x="131.86" y="263.717" width="91.286" height="91.287" />
      <rect x="40.569" y="40.568" width="91.286" height="91.287" />
      <rect x="223.139" y="40.568" width="91.286" height="91.287" />
      <rect x="40.569" y="223.141" width="91.286" height="91.287" />
      <rect x="223.139" y="223.141" width="91.286" height="91.287" />
      <rect width="139.285" height="139.285" transform="matrix(0.7071 -0.7071 0.7071 0.7071 78.569 177.502)" />
    </svg>
  );
}
