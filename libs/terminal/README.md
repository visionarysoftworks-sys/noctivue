# terminal

ANSI colors (truecolor and the 256-color palette), text attributes,
OSC 8 hyperlinks, terminal-cell width measurement, tables, spinners,
and progress bars — as pure, composable values. No TTY, no sleeping,
no environment.

Run the tests:

    noct test

Or directly:

    noct run tests/terminal_test.nv

Example:

```nv
import style
import table
import spinner

fn main():
    let head = style.with_bold(style.with_fg(style.plain(), style.color(255, 255, 255)))
    print(style.style_text("REPORT", head))
    // A palette color, a hyperlink, and CJK in one cell.
    let warn = style.with_fg(style.plain(), style.color_palette(214))
    print(style.hyperlink(style.style_text("警告", warn), "https://example.com/ja"))
    print(table.render_table(["item", "qty"], [["apple", "3"], ["fig", "12"]], ["left", "right"]))
    print(spinner.spinner_line(spinner.new_spinner(), "working"))
```

Every function returns a value and never prints, so you decide what to
emit and when. Widths are measured in **terminal cells on the stripped
text** — a styled cell, a hyperlinked cell, and a CJK cell all still
line up, because escape sequences are invisible and CJK is two columns
wide.

See `docs/overview.md` for the color API, the width table and its
documented approximations, and the OSC 8 framing.
