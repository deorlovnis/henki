# notify: a desktop notification when a turn ends or henki asks before a call

An observer (`on/turn_end`, `on/question`) for you, not for the model (it costs no prompt bytes). It shows "turn ended:
STATUS" or "asks before a NAME call", titled with the folder's name, through `osascript` on a Mac and `notify-send` on
Linux; without either it does nothing. On Linux it needs the desktop session: add
`pass_env = ["DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS"]` to henki's config (the tool calls get them
too). There is no terminal bell, because an observer has no terminal.
