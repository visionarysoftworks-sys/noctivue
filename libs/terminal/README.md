# terminal

ANSI colors, text attributes, tables, spinners, and progress bars —
as pure, composable values. No TTY, no sleeping, no environment.

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
    print(table.render_table(["item", "qty"], [["apple", "3"], ["fig", "12"]], ["left", "right"]))
    print(spinner.spinner_line(spinner.new_spinner(), "working"))
```

Output:

```text
REPORT
│ item  │ qty │
├───────┼─────┤
│ apple │   3 │
│ fig   │  12 │
└───────┴─────┘
⠋ working
```

Every function returns a value and never prints, so you decide what to
emit and when. Widths are measured on the *stripped* text, so a styled
cell still lines up.
