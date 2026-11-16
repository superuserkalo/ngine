---
name: ui-hillclimb
description: Improve ngine's window by rendering every screen, judging it against a strict rubric with fresh eyes, fixing the worst problems, and repeating. Use for any change to crates/ngine/src/ui or the screens it draws.
---

# UI hill climb

The bar is software a careful human designer shipped: lean, quiet, exact, in the spirit of Superwhisper's design language and at home in Omarchy's tiled desktop. Every pixel is a decision.

Climb from the window as it is. Its structure is the owner's accepted starting point: improve it step by step and never start a redesign. A step that makes one screen better and none worse is progress; a step that changes everything at once is not.

## The loop

1. **Render.** Run every snapshot scene (`cargo run -p ngine -- --snap`, see `src/app/demo.rs`) at 1280×800, 960×1080, 960×540 and 720×480 into `.scratch/hillclimb/round-N/`. Also render the real library and editor with `--library-of` and `--editor-on` on a real recording.
2. **Judge with fresh eyes.** Spawn a reviewer subagent that has NOT seen the code or your reasoning. Give it only this rubric and the round's PNG paths, and ask for every defect with the scene, the exact place, and how bad it is (blocker, clear flaw, nit). Look at every PNG yourself too.
3. **Fix the worst first.** Blockers, then clear flaws. Nits only when nothing worse is left.
4. **Repeat** until a round finds no blocker and no clear flaw, or three rounds have run without progress. Keep each round's pictures so the owner can see the climb.
5. **Drive the real app** on this Mac for what pictures cannot show: hover, focus, keyboard, menus, timing.
6. **Learn from misses.** When the owner finds a problem the rubric did not catch, add the rule that would have caught it to the rubric below, in the same commit as the fix.

## Rubric

**Say it once.** No heading that repeats the sidebar item or the window's purpose. No label above a control that says what the control already says. No status text that tells the user what they can already see ("Ready", "Idle"). No taglines or explanations under options unless a user would otherwise choose wrong.

**Words.** The words other recorders and editors use: "Trim", "Export", "Keyframe", "Re-encode", "Full screen". Sentence case. No made-up pairs like "Fast / Exact". Numbers with units. Nothing that reads like generated filler.

**The right control.** A small fixed set of choices is a segmented control or a menu. A value people set to their own number (a length, a bitrate, a folder) gets a field, with presets only when a few values cover nearly everyone. A switch only for on/off that takes effect at once. Keybinds are recorded by pressing them, shown as the system writes them, and can be cleared and reset.

**Every input is choosable.** Anything ngine records from (screen, window, app, microphone, system sound, output device) can be chosen where it is shown, defaults to the system's choice, and says plainly when a remembered device is missing.

**Hierarchy.** One primary action per screen, obviously so. Secondary actions quieter. Related things grouped; unrelated things separated by space, not boxes. No card inside a card. No borders where spacing does the job.

**Grid and spacing.** A 4 px grid. One set of spacing steps (4, 8, 12, 16, 24, 32) used consistently. Left edges align down the page. Equal gaps between equal things. Controls in a row share one height. Text baselines line up across a row.

**Type.** At most three sizes on a screen. Weight and tone carry hierarchy before size does. Muted text is still readable.

**Space.** Content uses the window: no large dead areas next to cramped ones, nothing cut off at 720×480, nothing stretched thin at 1280×800. Long names truncate with an ellipsis, never collide.

**Window frame.** No stock system title bar on any system: on Windows no white caption bar with the app name, on macOS the traffic lights sit inside ngine's own top strip, under Hyprland nothing at all. The top strip drags the window, double-click behaves as the system does, and no content collides with the window controls.

**Live state.** Recording and replay state is shown where it matters, small and calm, and only when something is live. Meters move when there is sound.

**States.** Hover, pressed, focus and disabled are visible and consistent. Disabled controls say why when hovered. Empty, loading and error states look designed, not forgotten.

**Tells of AI design to remove.** Status pills nobody asked for, emoji, gradients on everything, every element in its own rounded card, icons on every label, centred everything, redundant section titles, helper text under every row, three equal buttons where one matters.
