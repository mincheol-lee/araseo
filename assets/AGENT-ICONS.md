The terminal tab icons are local SVG assets so they render without a network connection.

- `codex*.svg`: Codex icon from [Lobe Icons](https://github.com/lobehub/lobe-icons), downloaded from `@lobehub/icons-static-svg`.
- `claude*.svg`: Claude icon from [Simple Icons](https://github.com/simple-icons/simple-icons), downloaded from `simple-icons@v16`.

The GPUI frontend embeds `claude.svg` and `codex.svg` in a static terminal-tab
icon view. Process detection selects the logo, and process exit removes it.
No hook configuration, activity tracking, or rotation animation is used.
