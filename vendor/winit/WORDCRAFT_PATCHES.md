# Local patches to winit 0.30.13 (Android)

Base: `storytold/winit` at `a517b3846ee8457d84ee4975110bfda6797c44dc`, vendored unchanged except for
the files below. Only `src/platform_impl/android/mod.rs` changed.

## 1. Keyboard modifiers (Ctrl, Shift, Alt, Meta)

Android never sent `WindowEvent::ModifiersChanged`, so the app never knew Ctrl or Shift was held:
`Ctrl+S` arrived as a plain `S`. Now every key event first reports the held modifiers, from the
event's meta state and from the modifier key itself (`modifiers_for_key`).

## 2. Mouse as a pointer

Motion events from a mouse (`ToolType::Mouse`) were ignored (`// TODO mouse events`). Now they go
through `mouse_motion`:

- `CursorMoved` for every move and hover;
- `MouseInput` for the buttons (primary, secondary, tertiary, back, forward), by change since the
  last event, so a press or release is never lost;
- `MouseWheel` for `ACTION_SCROLL` (horizontal and vertical).

Touch and stylus input are unchanged.

## Not yet done

- Touch as a pointer (planned phase 6): the touch path still reports `Touch` events only.
- Button events that carry no motion (`ButtonPress`) use the last pointer position.

Upstream: these changes are Android-only and could be offered to winit upstream once tested.
