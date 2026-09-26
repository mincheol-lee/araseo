The terminal tab icons are local SVG assets so they render without a network connection.

- `codex*.svg`: Codex icon from [Lobe Icons](https://github.com/lobehub/lobe-icons), downloaded from `@lobehub/icons-static-svg`.
- `claude*.svg`: Claude icon from [Simple Icons](https://github.com/simple-icons/simple-icons), downloaded from `simple-icons@v16`.

Numbered files rotate each source SVG in 45-degree steps. The application selects a frame at runtime because its Slint software renderer does not draw image rotation transforms.
