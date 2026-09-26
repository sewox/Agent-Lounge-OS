# Self-hosted fonts

These files replace `next/font/google` so production builds (Turbopack) do not
need network access to `fonts.googleapis.com` / `fonts.gstatic.com`.

| Family | Files | Weights | Source | License |
| --- | --- | --- | --- | --- |
| Geist Sans | `Geist-Variable.woff2` | 100–900 (variable) | [vercel/geist-font@1.8.0](https://github.com/vercel/geist-font/releases/tag/1.8.0) | `Geist-OFL.txt` (OFL 1.1) |
| IBM Plex Sans | `IBMPlexSans-{Regular,Medium,SemiBold}.woff2` | 400, 500, 600 | [`@ibm/plex@6.4.1`](https://www.npmjs.com/package/@ibm/plex) complete woff2 | `IBMPlexSans-OFL.txt` (OFL 1.1) |
| JetBrains Mono | `JetBrainsMono-{Regular,Medium,SemiBold,Bold}.woff2` | 400, 500, 600, 700 | [JetBrainsMono@2.304](https://github.com/JetBrains/JetBrainsMono/releases/tag/v2.304) | `JetBrainsMono-OFL.txt` (OFL 1.1) |
| Public Sans | `PublicSans-{Regular,Medium,SemiBold}.woff2` | 400, 500, 600 | [public-sans@v2.001](https://github.com/uswds/public-sans/releases/tag/v2.001) | `PublicSans-OFL.txt` (OFL 1.1) |

CSS variables (`--font-geist-sans`, `--font-ibm-plex`, `--font-jetbrains`,
`--font-public-sans`) are unchanged; see `src/app/layout.tsx`.
