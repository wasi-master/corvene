#!/usr/bin/env python3
"""A stand-in for oo7's CLI prompter (org.freedesktop.secrets.CliPrompter)
for packaging/linux/keyring-test-oo7.sh: oo7-daemon calls it when an
application on a session without a graphical seat asks to unlock a locked
collection. It answers the way it is told:

  oo7-prompter.py answer <password>   hand the daemon this password
  oo7-prompter.py dismiss             the user pressed Cancel
  oo7-prompter.py hang                never answer (a prompt nobody sees)

Needs python3-dbus and python3-gi.
"""

import socket
import sys

import dbus
import dbus.mainloop.glib
import dbus.service
from gi.repository import GLib

NAME = "org.freedesktop.secrets.CliPrompter"
PATH = "/org/freedesktop/secrets/CliPrompter"


class Prompter(dbus.service.Object):
    def __init__(self, bus, mode, password):
        super().__init__(bus, PATH)
        self.mode = mode
        self.password = password

    @dbus.service.method(
        NAME,
        in_signature="ss",
        out_signature="hb",
        async_callbacks=("reply", "error"),
    )
    def Prompt(self, label, description, reply, error):
        print(f"prompted: {label}: {description}", flush=True)
        if self.mode == "hang":
            return
        ours, theirs = socket.socketpair()
        if self.mode == "answer":
            ours.sendall(self.password.encode())
        ours.close()
        reply(dbus.types.UnixFd(theirs.fileno()), self.mode == "answer")
        theirs.close()


def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else "answer"
    password = sys.argv[2] if len(sys.argv) > 2 else ""
    dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
    bus = dbus.SessionBus()
    _name = dbus.service.BusName(NAME, bus)
    _prompter = Prompter(bus, mode, password)
    print(f"prompter ready ({mode})", flush=True)
    GLib.MainLoop().run()


if __name__ == "__main__":
    main()
