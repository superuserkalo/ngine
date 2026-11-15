# Audio is system sound and microphone in three tracks

Status: accepted on 2026-11-15

ngine records system sound and the microphone. Each has its own volume and can be switched off.

## Tracks

A recording holds three audio tracks: the mix first, then system sound, then the microphone.

- The mix is first because most players, and Discord, play only the first track.
- The separate tracks let the clip editor mute or rebalance the microphone afterwards.
- An exported clip always holds one mixed track.
- A setting records the mix only, for smaller files. The three tracks cost about 2 MB per minute.

Rejected:

- One mixed track only. The microphone could never be removed from a clip.
- Two tracks, system and microphone. The microphone would be silent in most players.

## Codecs

AAC by default, because it plays everywhere. Opus as a setting. Both at 48 kHz stereo.

## Sync

- Every video frame and audio buffer carries a timestamp from the capture session's clock. Nothing is ordered by arrival.
- The microphone and the system output run on separate hardware clocks. The microphone is resampled slightly to stay aligned over a long recording.
- Silence is written when a source delivers nothing.

## App-only sound

Recording one app or window records that app's sound by default, with a switch for all system sound. Recording a screen records all system sound.

On Linux the portal does not say which app a window belongs to, so the user picks the app from a list of what is playing.

## Where it comes from, and the minimum systems

- macOS: ScreenCaptureKit, which needs macOS 15 to deliver the microphone in the same session.
- Windows: WASAPI, which needs Windows 10 version 2004 for app-only sound.
- Linux: PipeWire. A system without PipeWire is not supported.

## Left out

Microphone noise suppression and push-to-talk.
