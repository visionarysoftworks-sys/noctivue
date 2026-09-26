# terminal — overview

Status: v0.1.0, native tier, pure `.nv` (no builtins beyond
`print`/`println`/`assert`, so it runs on `run` today and stays portable
to `run-vm`/`build`).

## What it does

- `style.nv` — `Color` and `Style` as values, SGR rendering, the 16
  conventional color names, `strip_ansi`, `visible_width`.
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
testable with no TTY", and it is why every function here is testable in a
plain assert test — the test file asserts rendered bytes, not pixels.

## Why truecolor only

Colors are RGB and always emitted as SGR `38;2;r;g;b` (`48;2;…` for
backgrounds). The 16-color SGR forms (`30`–`37`) are deliberately not
emitted: which RGB a terminal shows for "color 1" is the terminal's
policy, so honoring it is the terminal's job. `color_named` maps the 16
conventional names to canonical RGB so callers can still ask for "red".

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
end — so the loop cannot overrun, on any input. `check_non_ascii_is_safe_and_counted_in_characters`
is the regression guard; it asserts `"│x│".length == 7` and that its
visible width is 3.

This is worth fixing **in the language**, not here: a char-length
accessor (or making `length` characters with a separate `byte_length`)
would remove a trap that every future text-processing package will hit.

## Known limitations (v0.1.0)

- `visible_width` counts characters, not terminal cells. Correct for ASCII
  and for box-drawing characters; wrong for double-width text (CJK,
  emoji), which needs a wcwidth table.
- No hyperlink, OSC, or 256-color palette support (only 16 names mapped to
  RGB, plus arbitrary RGB).
- No column wrapping or truncation: a cell longer than the terminal is
  printed as-is, because guessing where to break is the caller's call.
- Tables cannot be built from computed data yet — lists are fixed-size
  (no push/concat, NIR.md §6), so callers pass literals. `table.nv` also
  recomputes each column's width where it uses it for the same reason;
  that is O(columns × rows) per line and irrelevant at CLI sizes.
- A live spinner needs the caller to own the redraw loop, so this package
  ships no `run_task`-style animation.

## Gates (from the package directory)

    noct diagnostics lib/*.nv tests/*.nv     # clean
    noct lint lib/*.nv tests/*.nv            # clean
    noct fmt --check lib/*.nv tests/*.nv    # clean
    noct test                                # 1 passed
    noct publish --dry-run                   # ready (unsigned, no deps)
