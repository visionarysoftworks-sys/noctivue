# terminal — overview

Status: v0.2.0, native tier, pure `.nv` (no builtins beyond
`print`/`println`/`assert`, so it runs on `run` today and stays portable
to `run-vm`/`build`).

## What it does

- `width.nv` — the wcwidth table: how many terminal cells a character
  occupies (0, 1, or 2).
- `style.nv` — `Color` and `Style` as values, SGR rendering, the 16
  conventional color names, the 256-color palette, OSC 8 hyperlinks,
  `strip_ansi`, `visible_width`.
- `table.nv` — box-drawing tables: per-column widths, left/right/center
  alignment, ragged and over-wide rows, header separator.
- `spinner.nv` — a spinner as an index into a frame list, advanced by
  the caller.
- `progress.nv` — bar and percentage rendering, with `Err` for impossible
  input.

## The one design decision everything else follows from

**Nothing here touches a TTY, a clock, or the environment.** A style is
a value; a bar is a value; a spinner is an index. The caller decides
when to redraw, whether to use `\r`, and when to sleep (`sleep` is
`run`-only anyway). That is the catalog's "styles as composable values,
testable with no TTY", and it is why every function here is testable in
a plain assert test — the test file asserts rendered bytes, not pixels.

## Colors: truecolor and the 256-color palette

A color carries EITHER RGB components or a palette index, and renders as
SGR `38;2;r;g;b` (or `48;2;…`) or `38;5;N` (or `48;5;N`) accordingly. The
16-color SGR forms (`30`–`37`) are deliberately never emitted: which RGB
a terminal shows for "color 1" is the terminal's policy, so honoring it
is the terminal's job.

`palette_rgb(i)` is the index → RGB mapping:

| range | layout |
| --- | --- |
| 0–15 | the 16 xterm *system* colors — a terminal may override these, so this is a reference, not a promise |
| 16–231 | the 6×6×6 cube; index − 16 is `r*36 + g*6 + b`, and level *N* is one of 0, 95, 135, 175, 215, 255 — **not** evenly spaced, which is why it is a table and not a formula |
| 232–255 | the 24-step gray ramp, `8 + 10 * (i − 232)` |

### The `Color` API shape (v0.2.0)

```nv
export struct Color:
    r: Int
    g: Int
    b: Int
    index: Option<Int>   // added in v0.2.0
```

`index` is the palette discriminator, and it is `None` for an RGB color.
So a `Color` is a **tagged union**: `index: Some(_)` means "render
`;5;N` and ignore the components", `None` means "render `;2;r;g;b`".

Constructors, all of which keep working exactly as they did in v0.1.0:

- `color(r, g, b)` — unchanged, clamps components into 0–255, `index` is `None`.
- `color_named(name)` — unchanged, the 16 names as canonical RGB.
- `color_palette(i)` — **new**; `index: Some(clamp(i))`, components 0.
- `color_named_palette(name)` — **new**; the 16 names as indices 0–15.
- `palette_rgb(i)` — **new**; `Option<Color>`, the inverse mapping, `None` outside 0–255.
- `resolve_color(c)` — **new**; collapse a `Color` onto the RGB side.

`color_palette` clamps rather than returning an `Err`, matching what
`color` already does with components: a palette index is usually
arithmetic (`16 + 36 * r`), and a total function is worth more than an
error path. When the index actually matters, validate with
`palette_rgb(i)` first, which *does* answer "is this a real slot?".

**Can a palette color and an RGB color coexist in one `Style`? Yes** —
`fg` and `bg` are independent fields, so `38;5;33;48;2;20;20;20` is a
perfectly good single escape. Within one `Color` they are mutually
exclusive, because the field is a union; `resolve_color` is the explicit
way across.

> **Breaking change.** A *direct* struct literal must now name all four
> fields. A literal missing a field is a distinct incomplete struct type
> in this language, so `Color { r: …, g: …, b: … }` written by hand no
> longer type-checks as a `Color`. Code that goes through the
> constructors is unaffected.

## Width: `visible_width` counts cells, not characters

`visible_width(s)` strips control sequences and then sums **terminal
cells**: CJK and fullwidth forms are 2, combining marks are 0, ASCII,
box drawing and block elements are 1. That is a real change from v0.1.0,
which counted characters and was therefore wrong for anything that is not
ASCII (it happened to be right for box drawing).

`width.char_cells(c)` is the table, and it is a **hand-written subset of
`wcwidth(3)`**, not a generated Unicode Character Database table. It is
shaped after Markus Kuhn's original (1999), with the East Asian
Wide/Fullwidth and emoji blocks terminals actually render double-width
added since:

- **0 cells** — C0/C1 controls, plus the combining-mark, bidi-control,
  variation-selector and default-ignorable blocks for the scripts a
  terminal realistically renders.
- **2 cells** — East Asian Wide and Fullwidth (Hangul, the CJK
  ideograph blocks, fullwidth ASCII) and the emoji blocks.
- **1 cell** — everything else, ASCII included.

### The approximations, stated plainly

1. **East Asian Ambiguous is 1 cell.** That is the near-universal
   Western terminal default; a CJK-locale terminal draws `│` and `█` as
   2 cells. Deliberate: guessing wrong for every Western user to be right
   for CJK-locale users is the wrong trade, and the alternative needs to
   ask the terminal.
2. **No grapheme clustering.** A flag is two regional indicators and a
   ZWJ family is several code points, so both measure *per code point*:
   a flag measures 4, not 2. This is pinned by a test so it cannot
   change silently.
3. **Emoji are 2 cells.** A character with default emoji presentation
   (U+1F300–U+1F6FF, U+1F900–U+1FAFF, the regional indicators, and the
   supplementary planes) is 2. A *text*-presentation symbol is 1 —
   unless it is followed by VS16 (U+FE0F), which promotes it to 2, the
   way terminals do: `☺` is 1 cell, `☺️` is 2.
4. **Not exhaustive.** A combining mark in a script whose block is not
   listed measures 1 instead of 0, shifting a column by one. The real fix
   is a table generated from `UnicodeData.txt` / `EastAsianWidth.txt`,
   which the language cannot embed yet (no file reads, no codegen step in
   the build).
5. **Control characters are 0 cells.** `wcwidth` returns −1 for these;
   this package clamps to 0, because a control has no glyph and its
   effect on the cursor is terminal-specific.

## OSC 8 hyperlinks

```text
hyperlink_open(url)   // ESC ] 8 ; ; url ESC \
hyperlink_close()     // ESC ] 8 ; ; ESC \
hyperlink(text, url)  // open + text + close
```

The `params` field is emitted empty. An empty URI *is* the close form
per the spec, so `hyperlink_open("") == hyperlink_close()`. Everything
after the second `;` is the URI, so a `;` inside a URL is safe.

`strip_ansi` had to grow to match: it used to understand only CSI
(`ESC [`), so an OSC 8 link would have been left in the string and
measured as visible text. It is now a small state machine over both
introducer families — CSI, and OSC terminated by ST (`ESC \`) or BEL
(0x07, which some programs emit). Its own tests cover the ST form, the
BEL form, an unterminated sequence, a `;` in the URL, an ESC inside the
payload that is *not* an ST, and CJK text inside a link.

A table cell containing CJK or a link still aligns, because widths are
measured on the stripped text. `check_table_measures_wide_and_linked_cells`
asserts those bytes exactly.

## The language fact that shaped this package

`String.length` is a **byte** length (UTF-8) while `s[i]` indexes
**characters** and panics past the character count. The two disagree, so
the obvious walk

```text
while i < s.length:      # bytes
    let c = s[i]         # characters
```

panics on any non-ASCII string. The stdlib has a bounds-safe
`string_char_at`, but it is **not reachable from a first-party library**
(no stdlib import path exists for one), so this package cannot use it.

Every string walk here is therefore **budgeted by bytes consumed**:

```text
var budget: Int = s.length        # bytes
while budget > 0:
    let c: String = char_at(s, i) # one character
    budget = budget - c.length    # that character's bytes
    i = i + 1
```

`i` walks characters, `budget` walks bytes, and they meet exactly at the
end — so the loop cannot overrun, on any input. With CJK and emoji in
scope this matters more than before: a 3-byte character is one character
but two display cells, so the two counters are pulling in different
directions.
`check_non_ascii_is_safe_and_counted_in_cells` is the regression guard;
it asserts `"│x│".length == 7` and that its visible width is 3.

This is worth fixing **in the language**, not here: a char-length
accessor (or making `length` characters with a separate `byte_length`)
would remove a trap that every future text-processing package will hit.

### Two more language facts, measured not guessed

- **`s[i]` is O(i)**, so every string walk here is O(n²) in the string
  length. At CLI sizes (a few hundred columns) that is invisible; at
  ~16 000 characters a single `visible_width` call took about 51 seconds.
  Nothing in this package can fix that, but callers should know before
  they measure a large buffer in a loop.
- **A table of ranges is not affordable.** `char_cells` was first written
  as `for r in ranges(): …` over a list of range structs, which measured
  at ~0.3 ms per character. The shipped version is a chain of range
  checks instead, which costs a handful of comparisons and is
  indistinguishable from free. This is why `lib/width.nv` is a long
  function and not a data structure.

## Known limitations (v0.2.0)

- The wcwidth table is a hand-written subset — see the approximations
  above. Grapheme clustering is absent, so multi-code-point emoji
  over-count.
- Only the 7-bit `ESC ]` OSC introducer is recognized by `strip_ansi`;
  the 8-bit C1 form (0x9D) is not.
- No column wrapping or truncation: a cell longer than the terminal is
  printed as-is, because guessing where to break is the caller's call.
- Tables cannot be built from computed data yet — lists are fixed-size
  (no push/concat, NIR.md §6), so callers pass literals. `table.nv` also
  recomputes each column's width where it uses it for the same reason;
  that is O(columns × rows) per line and irrelevant at CLI sizes.
- A live spinner needs the caller to own the redraw loop, so this package
  ships no `run_task`-style animation.
- Style parsing is not implemented: `strip_ansi` removes control
  sequences, it does not tell you what they said.

## Gates (from the package directory)

    noct diagnostics lib/*.nv tests/*.nv     # clean
    noct lint lib/*.nv tests/*.nv            # clean
    noct fmt --check lib/*.nv tests/*.nv    # clean
    noct test                                # 1 passed
    noct publish --dry-run                   # ready (unsigned, no deps)
