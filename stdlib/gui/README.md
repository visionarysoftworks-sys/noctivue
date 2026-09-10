# GUI Standard Library Foundation

This directory defines the platform-neutral data model for Noctivue GUI
programs.

| Module | Responsibility |
| --- | --- |
| `application.nv` | Application lifecycle and close-event handling |
| `window.nv` | Window configuration and state |
| `events.nv` | Window, keyboard, text, and pointer events |
| `canvas.nv` | Colors, geometry, and retained draw commands |
| `layout.nv` | Direction, spacing, insets, and content rectangles |
| `theme.nv` | Light and dark color palettes |
| `widget.nv` | Basic panel, label, editor, and button descriptions |

These modules describe GUI state and intent. They do not open native windows,
dispatch operating-system events, or render pixels. A future runtime backend
will consume these values and connect them to the target platform.

The modules are intentionally free-function based and avoid native handles.
That keeps Nightshade and other Noctivue applications independent of the
backend selected later.
