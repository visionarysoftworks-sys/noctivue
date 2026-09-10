# GUI-First Direction

Noctivue has not yet reached the point where it can create and render a
complete UI directly. Until the language and runtime support mature, the
project will first target GUI development.

## Foundation landed

The first platform-neutral GUI contract now lives in `stdlib/gui/`:

- `window.nv` defines window size, configuration, and lifecycle state.
- `events.nv` defines window, keyboard, text, and pointer events.
- `canvas.nv` defines colors, geometry, and basic retained draw commands.
- `layout.nv` defines direction, insets, spacing, and content rectangles.
- `theme.nv` defines reusable light and dark color palettes.
- `widget.nv` defines the first editor-oriented widget descriptions.
- `application.nv` defines application state and close-event handling.

These modules are deliberately backend-neutral. They model the data that a
native backend will consume; they do not claim to open a real OS window yet.
The next implementation step is a runtime backend that turns `WindowConfig`
into a native window and submits `DrawCommand` values to a platform surface.

This means prioritizing the tooling and runtime foundations needed for a
graphical application:

- window creation and lifecycle management
- input and event handling
- rendering surfaces and drawing primitives
- layout and widget composition
- application state and interaction
- platform integration

The GUI-first phase is an intermediate step, not a replacement for Noctivue's
eventual declarative UI goals. Once the language can reliably support these
capabilities, we can build higher-level UI syntax and reusable components on
top of the GUI foundation.
