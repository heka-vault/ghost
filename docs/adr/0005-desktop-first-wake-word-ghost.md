# 0005: Desktop first, with the local wake word "Ghost" in the MVP; mobile later

- **Status:** accepted
- **Date:** 2026-10-16

The first release is the Linux desktop: always-on local wake detection for "Ghost", a voice session, music ducking, and back to passive. Mobile (Expo, Android first, manual start) follows once the desktop loop works. Desktop needs no networking work because everything is on one machine; mobile needs a way to reach the house from cellular, which is decided when it starts.

## Consequences

- The wake word is trained with `livekit-wakeword` (synthetic data, ONNX output). A single short word wakes falsely more often than a two-word phrase, so the first spike compares "Ghost" with "Hey Ghost" against music before the phrase is final.
- The operator prefers not to open router ports or require a VPN client on the phone. Mobile reachability has to work within that, or record an explicit exception.
