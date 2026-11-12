/**
 * Notification permission, asked at the moment it means something: once pairing is done
 * ("so Ghost can reach you with the app closed"), and from Settings when it is off. Android
 * 13+ makes it a runtime permission; refused twice, only the system's settings can turn it on,
 * so the button goes there instead.
 */
import { useEffect, useState } from "react";
import { AppState, PermissionsAndroid, Platform } from "react-native";

import { type LinkPermissions, linkPermissions, openNotificationSettings } from "../../modules/ghost-call/link";

/** Ask for notifications (Android 13+); true when they are allowed afterwards. */
export async function askForNotifications(): Promise<boolean> {
  if (Platform.OS !== "android") return false;
  const now = linkPermissions();
  if (now?.notifications) return true;
  if (now?.needsRuntimePermission) {
    try {
      const r = await PermissionsAndroid.request(PermissionsAndroid.PERMISSIONS.POST_NOTIFICATIONS);
      if (r === PermissionsAndroid.RESULTS.GRANTED) return linkPermissions()?.notifications ?? true;
      if (r === PermissionsAndroid.RESULTS.NEVER_ASK_AGAIN) openNotificationSettings();
      return false;
    } catch {
      return false;
    }
  }
  // Before Android 13, or the permission granted but Ghost's channels turned off: the
  // system's own page is the only place to change it.
  openNotificationSettings();
  return false;
}

/**
 * The permissions behind the live connection, read again whenever the app comes back to the
 * front (the operator may have changed them in the system's settings), and on `refresh`.
 */
export function usePermissions(): [LinkPermissions | null, () => void] {
  const [p, setP] = useState<LinkPermissions | null>(() => linkPermissions());
  useEffect(() => {
    const sub = AppState.addEventListener("change", (s) => {
      if (s === "active") setP(linkPermissions());
    });
    return () => sub.remove();
  }, []);
  return [p, () => setP(linkPermissions())];
}
