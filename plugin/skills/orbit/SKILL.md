---
name: orbit
description: Open grok-orbit mission control — a pixel-art TUI where every Grok Build session is a rocket orbiting a planet. Working agents burn, waiting agents hold, finished agents land, crashed agents join the debris belt. Opens as a split pane next to this session when the terminal supports it.
argument-hint: ""
user-invocable: true
disable-model-invocation: false
---

# Launch mission control

The user wants grok-orbit running BESIDE this session — mission control in a
side pane, like a cockpit. It is a fullscreen TUI, so it must never run
inside this session or block it.

Prefer a split pane in the user's current terminal; fall back to a new
window. Check these in order and run the FIRST one that applies:

1. **tmux** (`TMUX` env var is set):
   `tmux split-window -h -l 45% grok-orbit`
2. **Windows Terminal** (`WT_SESSION` env var is set):
   `wt -w 0 split-pane --size 0.4 grok-orbit`
3. **macOS**:
   `osascript -e 'tell application "Terminal" to do script "grok-orbit"'`
4. **Linux, no tmux**: first of these that exists, backgrounded with `&`:
   `x-terminal-emulator -e grok-orbit`, `gnome-terminal -- grok-orbit`,
   `konsole -e grok-orbit`, `xterm -e grok-orbit`
5. **Windows, no Windows Terminal**:
   `powershell -NoProfile -Command "Start-Process grok-orbit"`

Execute the command with your shell tool — do not print it as text. After it
launches, reply with exactly one short line, e.g. "Mission control is live —
rockets on your right. 🚀" Do not explain what grok-orbit is unless asked.

If the command is not found, tell the user to install it with
`cargo install grok-orbit` (or grab a release binary from
https://github.com/trenbonole/grok-orbit) and try `/orbit` again.
