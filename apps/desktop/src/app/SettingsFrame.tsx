/**
 * Settings is one place with a page per topic (the operator's call, 2026-10-28): its own nav
 * to the left of the page, as in macOS System Settings and Linear, inside the page panel so
 * the app's sidebar stays as it is. The shell wraps every `area: "settings"` route in it; the
 * nav holds still while a page loads. A page with unsaved changes, a field to fix or a
 * restart owed carries a mark (settingsMarks.ts).
 */
import { RotateCw } from "lucide-react";
import type { ReactNode } from "react";

import { cx } from "../ui";
import { hrefFor } from "./router";
import { SETTINGS_PAGES } from "./routes";
import { type SettingsMark, useSettingsMarks } from "./settingsMarks";

const MARK_LABEL: Record<SettingsMark, string> = { dirty: "unsaved changes", restart: "restart to apply", error: "needs a fix" };

export function SettingsFrame({ current, children }: { current: string; children: ReactNode }) {
  const marks = useSettingsMarks();
  return (
    <div className="shell-settings">
      <div className="shell-settings__rail">
        <nav className="shell-settings__nav" aria-labelledby="shell-settings-label">
          <p className="shell-settings__label" id="shell-settings-label">
            Settings
          </p>
          <ul className="shell-settings__list" role="list">
            {SETTINGS_PAGES.map((p) => {
              const Icon = p.icon;
              const here = p.id === current;
              const mark = marks[p.id];
              return (
                <li key={p.id}>
                  <a className={cx("shell-settings__item", here && "is-current")} href={hrefFor(p.path)} aria-current={here ? "page" : undefined}>
                    <Icon size={16} aria-hidden />
                    <span className="shell-settings__item-label">{p.label}</span>
                    {mark === "restart" ? (
                      <span className="shell-settings__restart">
                        <RotateCw aria-hidden />
                        <span className="sr-only">, {MARK_LABEL.restart}</span>
                      </span>
                    ) : mark ? (
                      <span className={cx("shell-settings__mark", `is-${mark}`)}>
                        <span className="sr-only">, {MARK_LABEL[mark]}</span>
                      </span>
                    ) : null}
                  </a>
                </li>
              );
            })}
          </ul>
        </nav>
      </div>
      <div className="shell-settings__page">{children}</div>
    </div>
  );
}
