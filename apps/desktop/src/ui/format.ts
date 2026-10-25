/** Desktop-only formatting; everything shared with the phone lives in @ghost/ui. */
export { phaseOf, phaseText, agentName, lineName, relativeTime, formatTime, formatDateTime, errorText, quoted, statusLabel, statusTone } from "@ghost/ui";

const KEY_NAMES: Record<string, string> = {
  mod: "Ctrl", ctrl: "Ctrl", control: "Ctrl", cmd: "Super", meta: "Super", super: "Super", win: "Super",
  alt: "Alt", option: "Alt", shift: "Shift", enter: "Enter", return: "Enter", esc: "Esc", escape: "Esc",
  tab: "Tab", space: "Space", backspace: "Backspace", delete: "Delete", del: "Delete",
  up: "↑", down: "↓", left: "←", right: "→", arrowup: "↑", arrowdown: "↓", arrowleft: "←", arrowright: "→",
  pageup: "Page Up", pagedown: "Page Down", home: "Home", end: "End",
};

/** The keys of a shortcut in Linux names: shortcutKeys("mod+shift+z") is ["Ctrl", "Shift", "Z"]. */
export function shortcutKeys(shortcut: string): string[] {
  return shortcut
    .split("+")
    .map((k) => k.trim())
    .filter(Boolean)
    .map((k) => KEY_NAMES[k.toLowerCase()] ?? (k.length === 1 ? k.toUpperCase() : k[0].toUpperCase() + k.slice(1)));
}

/** A shortcut as text: formatShortcut("mod+z") is "Ctrl Z". Never ⌘; Ghost runs on Linux. */
export function formatShortcut(shortcut: string): string {
  return shortcutKeys(shortcut).join(" ");
}
