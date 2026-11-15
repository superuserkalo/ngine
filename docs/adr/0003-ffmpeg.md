# Encoding and the container go through a bundled FFmpeg

Status: accepted on 2026-11-15

ngine encodes video and audio, writes its files, and decodes for the clip editor with FFmpeg's libraries. They are trimmed to what ngine uses and ship inside the app on all three operating systems. The owner approved the LGPL dependency.

## Why

- It talks directly to the NVIDIA, AMD, Intel and Apple hardware encoders, with more control than Windows' Media Foundation gives.
- It covers H.264, HEVC and AV1. ngine offers only what the machine supports, with a software fallback when there is no hardware encoder.
- The user installs nothing extra.
- The clip editor needs a decoder for preview and for cuts that are exact to the frame.
- One code path serves three operating systems. Only macOS can be run by the people and agents building ngine today.
- Its MP4 writer has a crash-safe mode (`hybrid_fragmented`): fragments while recording, a normal MP4 after a clean stop. ngine writes no container code of its own.

Rejected:

- Each operating system's own encoder. That is three backends with fewer codecs and controls, and Linux has none of its own.
- The system's GStreamer on Linux. The user would have to have the right packages installed.

## Licence

- FFmpeg is built as LGPL 2.1 or later, without `--enable-gpl` and without `--enable-nonfree`. No GPL part, such as x264 or x265, goes in.
- It ships as separate shared libraries next to the app, never linked statically, with its licence text.
- The exact build script lives in this repository.

## What was measured

FFmpeg 9.0.2 was built on an Apple silicon Mac with only the VideoToolbox H.264 and HEVC encoders, AAC, decoders for H.264, HEVC, AAC and Opus, and MP4 and Matroska. The six stripped libraries total 3.8 MB and build in 24 seconds. The build used `--enable-small`.

Not measured: the Windows and Linux builds, and a software AV1 encoder.

## Rules that follow

- Recording is SDR. HDR is [issue #1](https://github.com/superuserkalo/ngine/issues/1).
- FFmpeg is a C library, so it sits behind one wrapper module that holds the `unsafe`.
- A captured GPU frame goes to the encoder without a CPU copy where the operating system allows it. On Linux the fallback is a CPU copy.

## When to revisit

If the bundled libraries pass about 10 MB on any operating system, or a store or platform makes the LGPL terms impossible to meet.
