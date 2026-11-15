/**
 * Every control Settings had before ADR 0073 (2026-11-14, eleven pages) and where it lives
 * now. Pure data: test/settings-structure.test.ts resolves each new address to a route, and
 * the PR's checklist is this list. Nothing was deleted; the Settings › Agents page went
 * because each thing on it is on the agents' own pages.
 */
export interface Moved {
  /** The page it was on. */
  was: string;
  control: string;
  /** Its address now, or "tray" for the tray's menu. */
  now: string;
  /** Where that is, in words. */
  where: string;
}

const m = (was: string, control: string, now: string, where: string): Moved => ({ was, control, now, where });

export const MOVED: Moved[] = [
  // General
  m("General", "Finish setup (while unfinished)", "/settings/general", "General"),
  m("General", "Set up Ghost (the first run again)", "/settings/general", "General"),
  m("General", "Float over the desktop", "/settings/general/floating", "General › Where Ghost appears"),
  m("General", "Ghost appears on (monitor)", "/settings/general/floating", "General › Where Ghost appears"),
  m("General", "Where on the screen", "/settings/general/floating", "General › Where Ghost appears"),
  m("General", "Captions", "/settings/general/floating", "General › Where Ghost appears, inside floating"),
  m("General", "While my screen is shared", "/settings/general/floating", "General › Where Ghost appears"),
  m("General", "Ghost on stage (Open the stage)", "tray", "The tray’s “Ghost on stage”"),
  m("General", "Version, build, desktop app", "/settings/about", "About"),
  m("General", "Settings file", "/settings/developer/advanced", "Developer › Advanced pages"),
  m("General", "Updates: check now, check for updates, update and restart, what’s new", "/settings/about/updates", "About › Updates"),
  m("General", "Error reports: send, the last 20 as sent", "/settings/general/reports", "General"),
  m("General", "Remove Ghost from this computer", "/settings/general/remove", "General"),
  // Model
  m("Model", "Every control of the model connection, its Test and its key", "/settings/model", "Model"),
  // Agents
  m("Agents", "Every agent, live state, Switch, New agent", "/agents", "Agents (the rail’s agents and +)"),
  m("Agents", "Ghost’s rules: Add rule, Open folder", "/agents", "Agents"),
  m("Agents", "What Ghost made (out/)", "/agents", "Agents"),
  m("Agents", "An agent’s voice, persona, look, memory, notes, history, retire, delete", "/agents/ghost/settings", "The agent’s own page"),
  // Passwords
  m("Passwords", "Every password by name, what uses it, last used", "/settings/developer/passwords", "Developer › Passwords"),
  m("Passwords", "Add, replace or delete a password", "/settings/developer/passwords", "Developer › Passwords, and where each is used"),
  m("Passwords", "Unlock the keyring, import plain files", "/settings/developer/passwords", "Developer › Passwords (the locked-keyring note leads there)"),
  m("Passwords", "The model’s key", "/settings/model", "Model, in place"),
  m("Passwords", "The realtime voice’s key", "/settings/sound/voice-mode", "Sound › Voice mode, in place"),
  m("Passwords", "An app’s key", "/apps/github", "The app’s page, in place"),
  // Skills
  m("Skills", "Every skill, add, remove, the catalog", "/settings/developer/skills", "Developer › Skills"),
  // Permissions
  m("Permissions", "What you’ve decided", "/settings/permissions", "Permissions"),
  m("Permissions", "Advanced: the mode, every rule, the policy file", "/settings/permissions/advanced", "Permissions › Advanced"),
  // Sound
  m("Sound", "Voice, wake phrases, sensitivity, your yes, wake takes", "/settings/sound/voice", "Sound"),
  m("Sound", "Listening, microphone, wake microphone, repair, speakers, echo cancellation", "/settings/sound/audio", "Sound"),
  m("Sound", "Voice mode (realtime), while Ghost speaks, voice models", "/settings/sound", "Sound"),
  // Phone
  m("Phone", "Pair a phone", "/settings/phone/pair", "General › Connections › Pair a phone"),
  m("Phone", "The paired phone, its addresses, forget it", "/settings/phone", "General › Connections › Your phone"),
  // Outside tools
  m("Outside tools", "Add or remove a client, its token, the listener", "/settings/developer/tools", "Developer › Outside tools"),
  // Notifications
  m("Notifications", "Ghost asks at the desk, desktop notification, the phone app", "/settings/general/reach", "General › How Ghost reaches you"),
  m("Notifications", "How long before the next way, quiet hours, calls limits, what is carried, the greeting, urgency, ntfy, call, SMS, WhatsApp", "/settings/developer/notifications", "Developer › Notification routes (linked from General)"),
  // Audit log
  m("Audit log", "Every change and its outcome, filters", "/settings/developer/audit", "Developer › Audit log (Today’s timeline shows the same)"),
  // The tray, until now its only place
  m("Tray", "Start at login", "/settings/general/login", "General, and the tray"),
];

/** The pages a newcomer sees, before and after. */
export const PAGES_BEFORE = ["General", "Model", "Agents", "Passwords", "Skills", "Permissions", "Sound", "Phone", "Outside tools", "Notifications", "Audit log"];
export const PAGES_AFTER = ["General", "Model", "Sound", "Permissions", "About"];
