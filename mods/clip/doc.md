# clipboard: clip-get prints it; echo TEXT | clip-put sets it

- `clip-get`: prints the clipboard's text.
- `clip-put`: sets the clipboard to its stdin (`echo TEXT | clip-put`, `clip-put < FILE`).

They use pbpaste and pbcopy on a Mac; on Linux wl-paste and wl-copy (Wayland), else xclip or xsel (X). Without one,
they say which to install. On Linux the calls need the display: add `pass_env = ["WAYLAND_DISPLAY", "DISPLAY"]` to
henki's config. With the halti mod on, `clip-get` is refused as `pbpaste` is, because the clipboard may hold a secret.
