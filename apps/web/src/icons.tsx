// The station's icons by name — Lucide, the set Silicon UI draws with — and the station's mark.
import {
  Activity, AppWindow, Archive, ArrowLeft, ArrowRight, BookOpen, Bell, Bug, Check, ChevronDown, Clock, Code, Columns2, Command, Copy, CopyPlus,
  Ellipsis, ExternalLink, FlaskConical, House, KeyRound, Keyboard, Link, List, LogOut, Moon, PanelLeft, Pause, Pencil, Pin, Play, Plus, RotateCw,
  Search, SlidersHorizontal, Sun, Table2, Trash2, User, X, type LucideIcon,
} from "lucide-react";

const ICONS: Record<string, LucideIcon> = {
  home: House, table: Table2, window: AppWindow, bell: Bell, settings: SlidersHorizontal, docs: BookOpen, search: Search, plus: Plus, x: X,
  back: ArrowLeft, forward: ArrowRight, reload: RotateCw, split: Columns2, more: Ellipsis, pin: Pin, chevron: ChevronDown, code: Code,
  key: KeyRound, panel: PanelLeft, bug: Bug, sun: Sun, moon: Moon, external: ExternalLink, copy: Copy, check: Check, user: User,
  logout: LogOut, keyboard: Keyboard, archive: Archive, trash: Trash2, link: Link, pencil: Pencil, play: Play, pause: Pause,
  pulse: Activity, clock: Clock, flask: FlaskConical, list: List, command: Command, duplicate: CopyPlus,
};

export function Icon(p: { name: string; size?: number; className?: string }) {
  const Glyph = ICONS[p.name];
  const size = p.size || 16;
  if (!Glyph) return <svg className={`icon ${p.className || ""}`} width={size} height={size} aria-hidden="true" />;
  return <Glyph className={`icon ${p.className || ""}`} size={size} strokeWidth={1.75} aria-hidden="true" />;
}

/** The station's mark: eight squares and the one turned 45°. */
export function Mark(p: { size?: number }) {
  return (
    <svg className="mark" viewBox="0 0 355 355" width={p.size || 18} height={p.size || 18} fill="currentColor" aria-hidden="true">
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
