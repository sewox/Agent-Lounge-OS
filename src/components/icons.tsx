export function Icon({ name, className = "h-3.5 w-3.5" }: { name: string; className?: string }) {
  const stroke = { fill: "none", stroke: "currentColor", strokeWidth: 1.6, strokeLinecap: "round" as const, strokeLinejoin: "round" as const };
  switch (name) {
    case "search":
      return (
        <svg viewBox="0 0 16 16" className={className} aria-hidden {...stroke}>
          <circle cx="7" cy="7" r="4.2" />
          <path d="M10.5 10.5 14 14" />
        </svg>
      );
    case "tune":
      return (
        <svg viewBox="0 0 16 16" className={className} aria-hidden {...stroke}>
          <path d="M2 4.5h12M2 11.5h12M5 3v3M11 10v3" />
        </svg>
      );
    case "terminal":
      return (
        <svg viewBox="0 0 16 16" className={className} aria-hidden {...stroke}>
          <rect x="1.5" y="2.5" width="13" height="11" rx="1.2" />
          <path d="m4 6 2.2 2L4 10M8.5 10.5H12" />
        </svg>
      );
    case "bell":
      return (
        <svg viewBox="0 0 16 16" className={className} aria-hidden {...stroke}>
          <path d="M8 2.5a3.5 3.5 0 0 1 3.5 3.5v2.2l1.2 2.3H3.3L4.5 8.2V6A3.5 3.5 0 0 1 8 2.5Z" />
          <path d="M6.5 13.2a1.6 1.6 0 0 0 3 0" />
        </svg>
      );
    case "stream":
      return (
        <svg viewBox="0 0 16 16" className={className} aria-hidden {...stroke}>
          <path d="M2 4h9M2 8h12M2 12h7" />
        </svg>
      );
    case "db":
      return (
        <svg viewBox="0 0 16 16" className={className} aria-hidden {...stroke}>
          <ellipse cx="8" cy="4" rx="5" ry="2" />
          <path d="M3 4v8c0 1.1 2.2 2 5 2s5-.9 5-2V4" />
        </svg>
      );
    case "health":
      return (
        <svg viewBox="0 0 16 16" className={className} aria-hidden {...stroke}>
          <path d="M2 8h3l1.5-3 3 6 1.5-3H14" />
        </svg>
      );
    case "chip":
      return (
        <svg viewBox="0 0 16 16" className={className} aria-hidden {...stroke}>
          <rect x="4" y="4" width="8" height="8" rx="1" />
          <path d="M6 2v2M10 2v2M6 12v2M10 12v2M2 6h2M2 10h2M12 6h2M12 10h2" />
        </svg>
      );
    case "chart":
      return (
        <svg viewBox="0 0 16 16" className={className} aria-hidden {...stroke}>
          <path d="M2 13h12M4 11V7M8 11V4M12 11V8" />
        </svg>
      );
    case "gear":
      return (
        <svg viewBox="0 0 16 16" className={className} aria-hidden {...stroke}>
          <circle cx="8" cy="8" r="2.2" />
          <path d="M8 2.5v1.4M8 12.1v1.4M2.5 8h1.4M12.1 8h1.4M4.1 4.1l1 1M10.9 10.9l1 1M11.9 4.1l-1 1M5.1 10.9l-1 1" />
        </svg>
      );
    case "folder":
      return (
        <svg viewBox="0 0 16 16" className={className} aria-hidden {...stroke}>
          <path d="M2.5 4.5h4l1.2 1.5H13.5v6.5H2.5z" />
        </svg>
      );
    case "book":
      return (
        <svg viewBox="0 0 16 16" className={className} aria-hidden {...stroke}>
          <path d="M3 3.5h4.5A2.5 2.5 0 0 1 10 6v7H5A2 2 0 0 0 3 15zM13 3.5H8.5A2.5 2.5 0 0 0 6 6v7h5a2 2 0 0 1 2 2z" />
        </svg>
      );
    case "key":
      return (
        <svg viewBox="0 0 16 16" className={className} aria-hidden {...stroke}>
          <circle cx="6" cy="8" r="2.4" />
          <path d="M8 8h6l-1.2 1.2M12 8v1.4" />
        </svg>
      );
    case "tree":
      return (
        <svg viewBox="0 0 16 16" className={className} aria-hidden {...stroke}>
          <path d="M8 2v5M5 14V9h6v5M4 7h8" />
        </svg>
      );
    case "rule":
      return (
        <svg viewBox="0 0 16 16" className={className} aria-hidden {...stroke}>
          <rect x="2.5" y="3" width="11" height="10" rx="1" />
          <path d="m5 8 2 2 4-4" />
        </svg>
      );
    case "pie":
      return (
        <svg viewBox="0 0 16 16" className={className} aria-hidden {...stroke}>
          <circle cx="8" cy="8" r="5.2" />
          <path d="M8 2.8V8l3.8 3.8" />
        </svg>
      );
    case "warn":
      return (
        <svg viewBox="0 0 16 16" className={className} aria-hidden {...stroke}>
          <path d="M8 2.4 14.2 13H1.8z" />
          <path d="M8 6.2v3.2M8 11.4h.01" />
        </svg>
      );
    default:
      return null;
  }
}
