# Instant replay keeps encoded video in memory up to a budget, then on disk

Status: accepted on 2026-11-16

While replay is on, ngine keeps encoding and holds only the last stretch of video and audio. A hotkey saves it as a clip. Replay is off until the user turns it on, because the encoder runs the whole time.

## The buffer

- It holds encoded video and audio, grouped from keyframe to keyframe. The oldest group is dropped whole.
- Saving copies from a keyframe into a temporary file, without re-encoding, and then renames the file into the library.
- It lives in memory while it fits a budget and on disk beyond that. A setting forces either.
- The budget is 3% of installed memory, never below 256 MB and never above 2 GB. The user can change it. It is a ceiling: a short buffer uses only what it holds.
- On disk it is short files in a folder the user can choose, deleted as they age. A save keeps the files it needs until it has finished.
- Length is 15 seconds to 30 minutes, 1 minute by default.

Rejected:

- Memory only. A 20-minute buffer at 1440p holds more than 2 GB next to the game.
- Disk only. The default 1-minute buffer would write to the SSD for nothing, and SSD wear is a common worry with disk-backed replay.
- Keeping MP4 fragments and joining them on save. Each fragment stores its time since the stream began, so a joined clip does not start at zero.
- One preallocated ring file, or a memory-mapped file. The first needs its own index and recovery format. The second needs `unsafe` and fails hard on a disk error.

## Rules that follow from what goes wrong elsewhere

- A clip is never shorter than asked. Replay encodes a keyframe every second and a save starts at the keyframe before the requested point, so a clip is at most 1 second long. The confirmation shows the real length.
- Replay never stops silently. When capture breaks, it restarts and keeps what it had, or tells the user.
- The bitrate is capped so the buffer's size is predictable. Settings show the cost in plain numbers.
- One encoder session feeds replay and recording. Starting a recording can include the buffered seconds before it.
- One buffer serves every save length: the last 30 seconds, the last N, or everything, each bindable and available from the command line.
- A setting clears the buffer after a save. Leftover files from a crash are removed at the next start.

## Markers

A hotkey during a recording drops a marker at that moment. The clip editor shows the markers on its timeline.

## Left out

A session-long background recording with a scrubbable timeline, as in Steam. A normal recording with markers covers it.

## What the decision rests on

- OBS and gpu-screen-recorder were read in source: both keep encoded packets and save by copying from a keyframe.
- gpu-screen-recorder and Medal offer memory and disk. OBS is memory only and Steam is disk only, and each has users asking for the other.
- The SSD ratings used to judge wear came from search summaries, not datasheets.
