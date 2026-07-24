#!/bin/zsh

# One shared PTY/VT text corpus for displayed comparisons through Mandatum's
# production native shell and a reference terminal. Selection remains a manual
# interaction, and the live cursor is supplied by each shell prompt afterward.

printf '\033[2J\033[H\033[0m'
printf 'MANDATUM WORK 3 — SHARED TYPOGRAPHY CORPUS\n'
printf 'ASCII    | The quick brown fox jumps over the lazy dog. 0123456789\n'
printf 'STEMS    | Il1|  O0QG  rn m  pq db  {}[]()  /\\  `'\''"_,.;:\n'
printf 'SYMBOLS  | -> => != <= >= === !== := :: && || ffi ffl  → ⇒ ≠ ≤ ≥ ± × ÷ ∑ ∫ √ ∞ • …\n'
printf 'BOX      | ┌────────┬────────┐  │ ║ ├ ┼ ┤  └────────┴────────┘\n'
printf 'FALLBACK | Ελληνικά Кириллица العربية עברית देवनागरी ไทย\n'
printf 'CJK      | 日本語  中文  漢字  界  한글\n'
printf 'COMBINE  | é  ä  ñ  Ż  ạ́  vs  é ä ñ Ż  (decomposed / precomposed)\n'
printf 'EMOJI    | 👩‍💻  👨‍👩‍👧‍👦  👍🏽  ❤️‍🔥  🏳️‍🌈  🚀  ✅\n'
printf '\n'
printf 'STYLES   | normal  '
printf '\033[1mbold\033[0m  '
printf '\033[2mdim\033[0m  '
printf '\033[3mitalic\033[0m  '
printf '\033[4munderline\033[0m  '
printf '\033[7minverse\033[0m\n'
printf 'COMBOS   | '
printf '\033[1;3mbold italic\033[0m  '
printf '\033[1;4mbold underline\033[0m  '
printf '\033[2;3;4mdim italic underline\033[0m  '
printf '\033[1;3;4;7mbold italic underline inverse\033[0m\n'
printf 'COLOR    | \033[31mred\033[0m \033[32mgreen\033[0m \033[33myellow\033[0m \033[34mblue\033[0m \033[35mmagenta\033[0m \033[36mcyan\033[0m \033[38;2;255;128;64mtruecolor\033[0m\n'
printf '\n'
printf 'SELECT   | Drag across this sentence in both windows for selection evidence.\n'
printf 'BASELINE | Hxgjpqy_  Hxgjpqy_  Hxgjpqy_  Hxgjpqy_\n'
printf '\n'
printf '\033[0mCURSOR   | Compare the live shell prompt cursor below.\n'
