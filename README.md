# grok-orbit 🚀

> Mission control for your [Grok Build](https://github.com/xai-org/grok-build) agents.
> Every session is a pixel-art rocket orbiting a planet in your terminal.

![demo](assets/demo.gif)

You're running four agents in parallel. One is refactoring, one is stuck waiting
for permission, one just finished, and one quietly died twenty minutes ago.
`grok dashboard` will tell you this in a table. **grok-orbit tells you this with
rockets.**

- 🔥 **Working agents burn** — orbiting fast, engines lit
- 🕐 **Waiting agents hold** — drifting slowly, engines cold
- ⚠️ **Permission-blocked agents flash `!`** — *awaiting launch clearance*
- ⛽ **The fuel gauge is your context window** — auto-compact is a mid-flight refuel
- 🛰️ **Subagents fly formation** as little probes next to their parent rocket
- ✅ **Finished agents land** on the planet
- 💥 **Crashed agents join the debris belt** — *Houston, we have a problem*
- 🪦 **The MISSION LOG remembers the last two weeks of flights**, with epitaphs
  (*"Ran out of context, and luck."*)

Zero configuration. Read-only. Your sessions are never touched, uploaded, or
even opened for writing — grok-orbit just watches the files grok-build already
writes.

## Install

```sh
cargo install grok-orbit
```

or from source:

```sh
git clone https://github.com/trenbonole/grok-orbit
cd grok-orbit
cargo run --release
```

### As a grok-build plugin (adds `/orbit`)

```sh
grok plugin install trenbonole/grok-orbit#plugin --trust
```

Then type `/orbit` inside any Grok Build session and mission control pops up
in a new terminal window. (The plugin needs the `grok-orbit` binary on your
PATH — `cargo install grok-orbit` handles that.)

No grok-build installed? Watch the demo fleet anyway:

```sh
grok-orbit --demo
```

(If you have no `~/.grok` at all, grok-orbit notices and flies the demo fleet
automatically — the first run is never a blank screen.)

## How it works

grok-build persists every session (TUI, headless, and ACP alike) under
`~/.grok/sessions/<encoded-cwd>/<session-id>/`. grok-orbit polls that tree
every 2 seconds — read-only, size-capped reads, mtime-cached — and maps what
it finds (sessions older than 14 days are ignored):

| grok-build reality                          | in orbit                          |
| ------------------------------------------- | --------------------------------- |
| `updates.jsonl` written < 12s ago            | 🔥 burning — fast orbit + flame    |
| quiet for < 15 min                           | 🕐 holding — slow drift            |
| last event is a permission request           | ⚠️ blinking `!` above the rocket   |
| quiet for > 15 min, last event looked fine   | ✅ lands on the planet             |
| last event carried an error                  | 💥 tumbles into the debris belt    |
| `signals.json` token counters                | ⛽ fuel gauge in the inspect card  |
| session with a `parent_session_id`           | 🛰️ probe flying formation          |

Statuses are heuristics over mtimes and the tail of `updates.jsonl` — grok-build
doesn't document these schemas (yet), so grok-orbit parses defensively and
degrades to "telemetry nominal" rather than crashing. If xAI changes the format,
worst case: your rockets get boring for a few days.

`GROK_HOME` is respected. Point it somewhere else with `--grok-home <path>`.

## Keys & mouse

| key         | action                                     |
| ----------- | ------------------------------------------ |
| `tab`       | switch between ORBIT and MISSION LOG       |
| `←` `→`     | select a rocket                            |
| click       | select the rocket under the cursor         |
| `esc`       | clear selection                            |
| `↑` `↓`     | browse the mission log                     |
| `f`         | filter the log (all / active / landed / lost) |
| `q`         | quit                                       |

## Flags

```
-d, --demo               fly a scripted demo fleet (no grok-build needed)
    --seed <N>           demo/starfield RNG seed (default: fixed, reproducible)
    --grok-home <PATH>   grok-build home to watch (default: $GROK_HOME or ~/.grok)
    --context-window <N> assumed context window for the fuel gauge (default 256000)
```

## FAQ

**Is this affiliated with xAI / SpaceXAI?**
No. Unofficial fan tooling. Apache-2.0'd grok-build made this possible;
grok-orbit is MIT.

**Does it send my code anywhere?**
No. There is no network code in this binary. It reads session metadata from
your disk and draws rockets. That's the whole thing.

**Does it work on Windows / macOS / Linux?**
Yes. It's built and tested on all three. You'll want a terminal with decent
Unicode + truecolor support (Windows Terminal, iTerm2, kitty, wezterm, ...).

**Why is my rocket "holding" when the agent is thinking?**
The heuristic is file-mtime based. Long silent thinking with no event writes
looks identical to waiting-for-input from the outside. When grok-build
documents a richer status surface, the rockets will get smarter.

**Can I keep it open all day on a second monitor?**
That is the intended use, yes.

## Roadmap

- [x] rockets
- [ ] sound effects (optional, tasteful, mostly beeps)
- [ ] `--acp` mode: attach as an ACP WebSocket client for real-time telemetry
- [ ] themes (deep space, retro green phosphor, synthwave)
- [ ] cease and desist from SpaceXAI legal

## Tech

Rust · [ratatui](https://ratatui.rs) 0.29 · crossterm 0.28 · serde_json ·
zero async, zero unsafe, one binary.

Inspired by [herdr-flock](https://github.com/ragamo/herdr-flock), which does
this for [herdr](https://github.com/ogulcancelik/herdr) with sheep. 🐑

## License

MIT
