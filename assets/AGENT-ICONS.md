The terminal tab icons are local SVG assets so they render without a network connection.

- `codex*.svg`: Codex icon from [Lobe Icons](https://github.com/lobehub/lobe-icons), downloaded from `@lobehub/icons-static-svg`.
- `claude*.svg`: Claude icon from [Simple Icons](https://github.com/simple-icons/simple-icons), downloaded from `simple-icons@v16`.

The GPUI frontend embeds `claude.svg` and `codex.svg` in the terminal-tab icon
view. Working agents rotate once per 800 ms, capped at 10 redraws per second.
Waiting agents keep their logo at its original angle. Inactive windows and the
system's reduced-motion setting disable rotation.

Numbered files are legacy 45-degree frames used by the earlier Slint frontend;
the GPUI view rotates the cached base icon directly.
