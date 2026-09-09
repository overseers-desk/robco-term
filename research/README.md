# Research

Verbatim user voices and competitive evidence about desktop terminal emulators, plus a record of what RobCo Terminal covers. The market files (voices, competitive-landscape, use-case-survey, discussion-drivers) are the basis for feature and launch copy, drawn from the field rather than from the code; copy should use words taken from `voices.md` so the project speaks to users in the language they already use. `status.md` is the one code-grounded file: it records which pains RobCo Terminal covers, where in the source, and the position that follows.

The folder is organized around the pains people voice about living with a terminal emulator, and what the field supplies for each. `use-case-survey.md` defines the pain taxonomy (P1-P13) and holds the canonical numbers; `voices.md`, `competitive-landscape.md`, and `discussion-drivers.md` map their evidence to those numbers, and `status.md` maps each pain to RobCo Terminal's coverage.

The folder was produced by the PLACE methodology (Poll, Landscape, Audit, Contrast, Establish), which lives in the aesop repository under `market-place/`; this folder is a run of it.

## Files

- `voices.md`: quotebook of user pain-points by category. Each entry is a verbatim quote (or a clearly marked paraphrase) with link, date, context, and the pain it maps to.
- `competitive-landscape.md`: a feature matrix of the terminals that ship a retro look, columns being the pain numbers, over a reference survey of the rest of the category and a per-pain supply note. Dated snapshot. RobCo Terminal is in neither; its coverage is in `status.md`.
- `use-case-survey.md`: the pain taxonomy and per-pain demand signal. Holds the canonical pain numbers. Dated snapshot.
- `discussion-drivers.md`: what a terminal launch draws on Hacker News, what pulls replies, and the specific precedent for a retro CRT terminal. Dated snapshot.
- `status.md`: RobCo Terminal's coverage of each pain with source paths and ceilings, the feature catalogue, the rarity of each capability against the field, the gaps, the known issues a reader of the code meets, and the positioning that follows. The only file written from the code.
- `performance.md`: what an idle terminal costs, where the cost sits, what each part is worth, and the traps that make a naive re-measurement disagree.
- `keybinding-formats.md`: how eight terminals let a user rebind the terminal's own chords, the nine decisions such a format cannot revisit once shipped, and the three places a product changed its format afterwards. Read for the `[bindings]` table's design; it sits outside the pain-number scheme, being a study of formats rather than of voiced pain.

## The pain-category numbering

Pain numbers are stable identifiers defined once in `use-case-survey.md`. They are not a ranking. When a category changes (added, removed, renumbered), update the cross-reference in `voices.md`, `competitive-landscape.md`, `use-case-survey.md`, and `status.md` together.

## Conventions

- Verbatim quotes get double quotes and an attribution line. The attribution line is the one place an em-dash is allowed.
- Verbatim user text is reproduced faithfully, including any punctuation inside the quote.
- Paraphrases are marked `[paraphrase]` with a link, so a future contributor can re-read the source and replace with verbatim text. Treat every paraphrase as a TODO for verbatim recovery.
- Every entry carries a working link and a date. Approximate dates are written `~2024`; precision is never invented.
- Absence of evidence for a claim is recorded as such ("no verbatim complaint found; closest is ..."), kept distinct from evidence of absence.
- Within a category, sort by source date, oldest first, so the corpus reads as a timeline.
- Engagement figures are the venue's own: Reddit and HN scores, GitHub reaction counts.

## Sources, and what was not reached

The August 2026 snapshot draws verbatim material from Hacker News (through the Algolia API), the GitHub issue trackers of cool-retro-term, Ghostty, WezTerm, kitty, Alacritty, Rio, Contour, Zellij and tmux (through `gh api`), Reddit (r/commandline, r/linux, r/unixporn, r/archlinux, r/KittyTerminal, r/wezterm, r/Fedora, r/linuxquestions, r/tmux, r/selfhosted, r/zellij, r/ClaudeCode, r/ClaudeAI, r/rust, r/kde, through the reddit.com skill's headless browser), Lobsters, The Register forums and developer blogs.

Not reached, recorded as absence rather than as evidence of absence: r/rust and r/kde yielded a handful of entries on a late serial sweep (the Alacritty and WezTerm announcement threads, a Wayland resize-lag thread); "terminal emulator" in r/kde surfaced project announcements, not complaints; the Reddit multi-session and tmux sweep (r/tmux, "too many tabs", "tmux -CC", "zellij vs tmux") returned nothing on its first pass because the one shared headless browser was saturated by the parallel retro-look sweep, and was re-run serially afterwards with the skill's two-step fetch, which returned 36 entries (in `voices.md` §5–§7, §13); "lose track of terminal tabs" and the r/linux "terminal multiplexer" listing still did not return. Reddit post scores and comment counts were fetched afterwards for the sixteen threads the voices came from, so `discussion-drivers.md` tabulates Hacker News as a venue search and Reddit as those sixteen threads. Mastodon and YouTube comments were not reachable. Every entry in `voices.md` is verbatim; the one paraphrase the collection carried (The Register forums) was recovered from the page.

Collection ran on Sonnet-tier agents per source, one per venue, under a fixed entry form; the taxonomy was authored by one reader over the merged corpus; the code audit ran on an Opus-tier agent with the source open; the competitor matrix was transcribed on a Sonnet-tier agent from the products' own pages, and one fabricated summary (a fetcher inventing CRT and tmux claims for a product that makes none) was caught and discarded before it reached the matrix.

## Growing the folder

Add new quotes to `voices.md` under the matching category. A new category is fine once it has at least two entries; until then keep the singleton in the closest sibling category. Re-snapshot `competitive-landscape.md`, `use-case-survey.md`, and `discussion-drivers.md` when the field changes meaningfully: a mainstream terminal ships tmux control mode on Linux, a new retro terminal reaches the top tier, or six months pass. Update `status.md` when the code changes, and in particular when any of the feasible gaps it lists closes.
