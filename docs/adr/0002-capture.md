# Capture is our own crate over one safe crate per operating system

Status: accepted on 2026-11-15

ngine captures through a capture crate of its own in this workspace. The crate does not depend on Rune or on the app, so it can be published for other projects once it works on all three operating systems.

## What it captures

- A whole screen, one window, or one app where the operating system supports it.
- System sound and the microphone.
- Video and audio come from one session and carry timestamps from one clock, so sync belongs to the crate.
- A frame carries the GPU surface. A CPU copy is made only when asked for, because the hardware encoder takes the surface directly.
- The system picker works on every operating system. Listing screens and windows works on macOS and Windows only, because Wayland does not let an app list windows.

## How it is built

Our API sits over one maintained safe crate per operating system: `screencapturekit` on macOS, `windows-capture` on Windows, and `ashpd` with `pipewire` on Linux. Each one stays behind our API and is replaced with our own code only where it blocks us.

Rejected:

- `scap`, the existing crate with this shape. Its repository has had no push since August 2025.
- Binding the operating-system APIs ourselves. Every call would be `unsafe`, and it repeats work the three crates have done.

## Linux

- Wayland comes first, through the screen-capture portal and PipeWire. That covers Hyprland (Omarchy), GNOME, KDE, Sway, niri and COSMIC.
- The portal shows its own picker. Its restore token is stored so the user is asked once.
- X11 is a second backend behind the same API, for desktops such as Cinnamon and Xfce.

## Other processes

The crate uses only the operating system's own capture interfaces, from ngine's own process, the way the built-in recorders do. It never reads from or injects into another process, and it does not hide itself from other software.

## When to revisit

If an inner crate stops being maintained, or cannot hand over the GPU surface without a copy.
