/**
 * The mock daemon's `GET /uninstall` (#147, `ghostd::uninstall::view`): what a used install
 * left, by kind, as the 2026.11.1 container's inventory found it, with sizes. Nothing here
 * removes anything.
 */
import type { UninstallView } from "@ghost/client-ts";

export function uninstallView(): UninstallView {
  return {
    install: "deb",
    finish: "sudo apt remove heka-ghost-desktop",
    categories: [
      {
        id: "running",
        title: "What keeps running or starts by itself",
        sentence:
          "The echo canceller's service, Ghost's entry in Start at login, and the GNOME Shell helper if you installed it. They are stopped and removed; your microphone and speakers go back to what the system chooses.",
        warning: null,
        persons: false,
        size: 1_204,
        items: [
          {
            key: "echo-cancel",
            what: "The echo canceller's service, its settings and the sound settings it added",
            paths: ["~/.config/systemd/user/ghost-echo-cancel.service", "~/.config/systemd/user/default.target.wants/ghost-echo-cancel.service", "~/.config/ghost/echo-cancel.conf", "~/.local/state/ghost/installed"],
            size: 1_013,
            note: null,
          },
          { key: "autostart", what: "Ghost in Start at login", paths: ["~/.config/autostart/heka-ghost-desktop.desktop"], size: 191, note: null },
        ],
      },
      {
        id: "other-tools",
        title: "Ghost's entries in your other tools",
        sentence: "Ghost's entry in Cursor, Codex or Claude Code, the copies Ghost made of their files before changing them, and the keys those tools use to reach Ghost.",
        warning: null,
        persons: false,
        size: 266,
        items: [
          { key: "in-claude-code", what: "Ghost in Claude Code", paths: [], size: 0, note: "Ghost does not write Claude Code's settings; run this yourself: claude mcp remove --scope user ghost" },
          { key: "in-cursor", what: "Ghost in Cursor", paths: ["~/.cursor/mcp.json.ghost-backup-20261114T225845"], size: 70, note: null },
          { key: "client-keys", what: "The keys your other tools use to reach Ghost", paths: ["~/.local/share/ghost/clients"], size: 196, note: null },
        ],
      },
      {
        id: "speech-models",
        title: "The speech models Ghost downloaded",
        sentence: "Parakeet and Kokoro, which Ghost downloaded to hear and speak. Ghost can download them again.",
        warning: null,
        persons: false,
        size: 2_842_118_400,
        items: [{ key: "speech-models", what: "Parakeet, Kokoro and Ghost's cache", paths: ["~/.cache/ghost"], size: 2_842_118_400, note: null }],
      },
      {
        id: "app-storage",
        title: "The app's own storage",
        sentence: "The window's own storage and cache, the app's log and choices, and the files Ghost keeps while it runs.",
        warning: null,
        persons: false,
        size: 1_402_880,
        items: [
          {
            key: "app-storage",
            what: "The window's storage and cache, the app's log and choices, Ghost's files while it runs",
            paths: ["~/.local/share/dev.heka.ghost", "~/.local/state/ghost", "$XDG_RUNTIME_DIR/ghost"],
            size: 1_402_880,
            note: null,
          },
        ],
      },
      {
        id: "data",
        title: "Your data",
        sentence: "Ghost's folder (settings, apps, rules, agents and their memory, skills, outputs), the database with every transcript and the audit, kept wake takes, the Hearing test's recordings and error reports kept here.",
        warning: "Your transcripts, agents, memory and settings are deleted and cannot be recovered. Kept wake takes are recordings of your voice; they go too.",
        persons: true,
        size: 61_874_176,
        items: [
          { key: "ghost-folder", what: "Ghost's folder: settings, apps, rules, agents and their memory, skills, outputs", paths: ["~/.config/ghost"], size: 48_640, note: null },
          { key: "database", what: "The database: every transcript, the audit, schedules and questions", paths: ["~/.local/share/ghost/ghost.db", "~/.local/share/ghost/ghost.db-wal"], size: 18_350_080, note: null },
          { key: "wake-takes", what: "Kept wake takes: recordings of your voice saying Ghost's name", paths: ["~/.local/share/ghost/wake/takes"], size: 43_401_216, note: null },
          { key: "data-folder", what: "Everything else in Ghost's data folder (the paired phone, latency records)", paths: ["~/.local/share/ghost"], size: 74_240, note: null },
        ],
      },
      {
        id: "vault",
        title: "Your passwords",
        sentence: "The password vault and its key in your login keyring.",
        warning: "Every password saved in Ghost is deleted and cannot be recovered.",
        persons: true,
        size: 2_048,
        items: [
          { key: "vault", what: "The password vault", paths: ["~/.local/share/ghost/passwords.vault"], size: 2_048, note: "Every password saved in Ghost is deleted and cannot be recovered." },
          { key: "keyring-key", what: "The vault's key in your login keyring (\"Ghost passwords vault key\")", paths: [], size: 0, note: null },
        ],
      },
    ],
  };
}
