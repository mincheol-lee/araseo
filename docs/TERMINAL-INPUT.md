# Terminal combination keys

Araseo encodes keys in the shared terminal input path, so Codex, Claude Code,
and shells receive the same terminal protocol bytes. No agent configuration or
user keybindings are changed.

## Encoding

- Arrows, Home/End, Insert/Delete, Page Up/Down, and F1–F12 preserve Shift,
  Alt, and Ctrl, including combinations. Xterm's modifier parameter is
  `1 + Shift + 2*Alt + 4*Ctrl`. For example, Shift+Left sends `ESC [ 1 ; 2 D`,
  Ctrl+Left sends `ESC [ 1 ; 5 D`, and Ctrl+Shift+Left sends `ESC [ 1 ; 6 D`.
- Plain Enter, Tab, Backspace, Escape, ordinary text, and familiar Ctrl-letter
  commands keep their existing shell encodings. Shift+Tab sends Backtab.
- Keys that cannot be distinguished with the legacy encoding use CSI-u.
  Examples: Shift+Enter is `ESC [ 13 ; 2 u`, Ctrl+Backspace is
  `ESC [ 127 ; 5 u`, and Ctrl+Shift+R is `ESC [ 114 ; 6 u`.
- Alt+Enter and Alt+Backspace retain their legacy Escape-prefixed encoding.
  Alt combinations that would look like incomplete CSI/OSC/SS3 control sequences
  (for example Alt+`]`) use CSI-u so the receiver can continue parsing input.
- Ctrl+W and Ctrl+S reach the focused terminal. Outside terminals, they still
  close an application tab and save the active file respectively.
- Ctrl+Tab / Ctrl+Shift+Tab still cycle application tabs; Ctrl+Shift+T creates
  a terminal. Ctrl+Shift+C copies terminal selection, and Ctrl+V / Shift+Insert
  paste. OS-reserved shortcuts remain subject to Windows handling.
- IME composition is owned by the native text input handler; preedit navigation
  is not sent to the PTY. Ordinary committed Unicode text stays UTF-8.

The terminal does not advertise full Kitty keyboard enhancement, release-event
reporting, or modifyOtherKeys negotiation. Unconditionally enabling those modes
without implementing their complete lifecycle could stall or misinterpret input.
CSI-u for otherwise ambiguous keys is also defined by Kitty's legacy mode.
Older shell line editors may require their own bindings for these extended keys.
Recognizing a key does not assign it an action: the running CLI's keymap decides
what the key does.

## Regression coverage

The core harness checks all seven modifier combinations for navigation,
editing, and function keys. The UI harness dispatches actual key events through
production GPUI input handlers and verifies bytes, focus, native IME handling,
and Ctrl+W/Ctrl+S precedence inside the workbench. The native Windows
`TerminalInput` fixture checks physical modifier keys in normal and alternate
screen buffers. It records PTY input without starting or authenticating agents.

## Primary references

- [Xterm control sequences: PC-style function keys](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h2-PC-Style-Function-Keys)
- [Kitty keyboard protocol: legacy event encoding](https://sw.kovidgoyal.net/kitty/keyboard-protocol/#legacy-key-event-encoding)
- [Codex's Crossterm parser: CSI modifiers and CSI-u](https://github.com/openai-oss-forks/crossterm/blob/45fecb9508105988f42fe6ff0441783ed3717f92/src/event/sys/unix/parse.rs)
- [Claude Code terminal configuration](https://code.claude.com/docs/en/terminal-config)
