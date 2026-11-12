// Ghost's desktop helper (ADR 0059, docs/floating-room.md). Optional: the floating room works
// without it, but on GNOME's Wayland nothing outside the compositor can tell where the pointer
// is or which window has the focus, so "the other monitor" falls back to the non-primary one.
//
// One D-Bus method on the session bus, dev.heka.Ghost.Desktop.Where(), returning JSON:
//   {"pointer":[x,y], "focus":[x,y,w,h]|null, "focus_fullscreen":bool, "workspace":n}
// in the stage's logical pixels. Positions only, never a title, a window's contents or its
// app; answered when asked (once per wake), nothing kept, nothing sent anywhere.
import Gio from 'gi://Gio';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const NAME = 'dev.heka.Ghost.Desktop';
const PATH = '/dev/heka/Ghost/Desktop';
const IFACE = `<node>
  <interface name="${NAME}">
    <method name="Where"><arg type="s" direction="out" name="json"/></method>
  </interface>
</node>`;

export default class GhostDesktop extends Extension {
    enable() {
        this._object = Gio.DBusExportedObject.wrapJSObject(IFACE, this);
        this._object.export(Gio.DBus.session, PATH);
        this._owner = Gio.bus_own_name(Gio.BusType.SESSION, NAME, Gio.BusNameOwnerFlags.NONE, null, null, null);
    }

    disable() {
        this._object?.unexport();
        this._object = null;
        if (this._owner)
            Gio.bus_unown_name(this._owner);
        this._owner = 0;
    }

    Where() {
        const [x, y] = global.get_pointer();
        const w = global.display.get_focus_window();
        let focus = null;
        let fullscreen = false;
        if (w) {
            const r = w.get_frame_rect();
            focus = [r.x, r.y, r.width, r.height];
            fullscreen = w.is_fullscreen();
        }
        return JSON.stringify({
            pointer: [x, y],
            focus,
            focus_fullscreen: fullscreen,
            workspace: global.workspace_manager.get_active_workspace_index(),
        });
    }
}
