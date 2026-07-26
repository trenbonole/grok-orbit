---
name: orbit
description: Open grok-orbit mission control — a pixel-art TUI where every Grok Build session is a rocket orbiting a planet. Working agents burn, waiting agents hold, finished agents land, crashed agents join the debris belt. Launches in a separate terminal window.
argument-hint: ""
user-invocable: true
disable-model-invocation: false
---

# Launch mission control

The user wants to watch their agents fly. Launch `grok-orbit` in a NEW
terminal window — it is a fullscreen TUI and must not block this session.

Pick the command for the current OS and run it:

- **Windows:** `powershell -NoProfile -Command "Start-Process grok-orbit"`
- **macOS:** `osascript -e 'tell application "Terminal" to do script "grok-orbit"'`
- **Linux:** try `x-terminal-emulator -e grok-orbit`, then `gnome-terminal -- grok-orbit`,
  then `konsole -e grok-orbit`, then `xterm -e grok-orbit` (first one that exists; append `&`).

After it launches, reply with exactly one short line, e.g. "Mission control
is live — go watch your rockets. 🚀" Do not explain what grok-orbit is
unless asked.

If the command is not found, tell the user to install it with
`cargo install grok-orbit` (or grab a release binary from
https://github.com/trenbonole/grok-orbit) and try `/orbit` again.
