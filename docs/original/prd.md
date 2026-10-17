# VoiceTriage — Product Requirements Document

**Version:** 1.0 (proposed)  
**Date:** 2026-10-16  
**Status:** Draft for implementation planning  
**Platforms:** Linux desktop; Android-first React Native mobile (iOS later)  
**Primary use case:** Conversational GitHub issue triage, architectural sign-off, and issue updates

## 1. Product summary

VoiceTriage is a voice-first engineering review assistant that helps a technical lead identify the GitHub issues that need decisions, discuss options, record approvals or change requests, and move work toward development—without switching to an issue tracker during the conversation.

It has **two deliberately different activation experiences**:

1. **Linux desktop:** an always-running **local wake-word detector** recognizes a customizable phrase (initially **“Hey Computer”**) even while music plays. Detection activates an interactive voice session, optionally ducks desktop music, and restores audio when the session ends. Idle microphone audio is not sent to a cloud model.
2. **Mobile:** the user **opens the React Native app and explicitly starts** a voice session. There is **no always-on listening or wake word on mobile**. The primary mobile situation is a hands-free conversation over the phone/car audio system, including while traveling. The user should start the session while safely parked, before driving, or with assistance; no interactions should demand looking at or touching the device while driving.

Both clients connect to the **same voice agent, GitHub integration, and persisted issue-review state**, so decisions and context can be resumed on either device. A calm, optional “thinking” sound provides feedback during longer operations. GitHub writes are permitted only through a defined, validated action flow with explicit spoken confirmation.

### Product thesis

The value is not simply “talking to GitHub.” It is **turning a scattered backlog into decisions that unblock engineering**, while preserving the rationale in GitHub and making the conversation feel natural.

## 2. Goals and non-goals

### Goals

- **G-01 — Identify decision bottlenecks:** answer “What are the top issues needing my sign-off before development?” with a grounded, prioritized shortlist.
- **G-02 — Discuss and decide by voice:** summarize issue context, read acceptance criteria, compare implementation choices, ask clarifying questions, and capture explicit decisions.
- **G-03 — Execute permitted updates:** add rationale comments, add/remove labels, change configured issue/project workflow status, and submit change requests using authenticated GitHub operations.
- **G-04 — Desktop ambient availability:** wake on a local phrase without interrupting music beforehand; remain quiet and private otherwise.
- **G-05 — Mobile intentional use:** quick manual session start, natural audio-only dialogue, support Bluetooth/car audio, reconnect when cellular connectivity degrades.
- **G-06 — Trustworthy outcomes:** every mutation is attributable, previewed, confirmed, auditable, and reported as succeeded/failed/partially applied.
- **G-07 — Model/provider flexibility:** support a managed realtime or STT–LLM–TTS pipeline without coupling GitHub domain logic to the model provider.

### Non-goals for v1

- Autonomous code writing, PR merging, deployments, shell commands, or actions on arbitrary websites.
- Mobile always-on wake-word listening; system assistant replacement; or Android Auto/head-unit integration.
- Fully offline GitHub operations (network access to GitHub is required).
- Unattended bulk approvals and silent modification of dozens of issues.
- Voice biometrics as the sole authorization factor.
- Broad GitHub permissions beyond an explicit repository allowlist.
- Phone interactions that require screen-reading, typing, or complex decisions while driving.

## 3. Personas and contexts

| Persona | Environment | Primary objective |
|---|---|---|
| Engineering lead / CTO | Linux workstation; music frequently playing | Review blockers and approve architectural decisions without interrupting concentration |
| Same lead on the move | Android app; Bluetooth headset or vehicle audio | Listen to succinct backlog summaries and give straightforward decisions hands-free |
| Repository collaborators (indirect) | GitHub issues and project board | Receive accurate decision rationale, clear scope, and unambiguous ready-for-development status |

**Design principle:** The voice experience is primary. Screen UI is supplementary for onboarding, settings, audit, reconnect, and exceptions.

## 4. Core user journeys

### J-01 — Desktop wake, triage, and resume music

1. User is working; music is playing through speakers or headphones. The desktop agent is idle, listening **locally only** for a wake word.
2. User says “Hey Computer.” The agent plays an optional chime, activates a session, and ducks the configured music stream if enabled.
3. User: “What needs my sign-off before anyone can begin development?”
4. Agent retrieves authorized issues, dependencies, comments, configured workflow fields, and related context. It gives a **short prioritized answer**, each item grounded in GitHub state.
5. User asks for more information or says “next issue”; agent maintains issue context and permits interruptions.
6. On “go to sleep,” “that’s all,” or idle timeout, the app stops the voice session and **restores exactly the previous music volume**; wake-word detection resumes.

### J-02 — Voice-only issue approval with write-back

1. Agent identifies `org/repo#142` as waiting for an architectural choice.
2. User discusses options, chooses one, and asks to approve the issue.
3. Agent **prepares but does not execute** a write proposal: target repository + issue number, rationale comment, labels/status to be changed, and any constraints.
4. Agent reads a compact confirmation: “Approve Redis for org/repo issue 142, add this rationale, and mark it ready for development. Confirm?”
5. User explicitly says “Confirm.” The application validates the pending proposal, identity/session, allowed operation, and current issue state.
6. On success, agent says exactly what changed, linking the decision to a persisted audit record. If one step fails, it reports the partial result and does **not** claim full success.
7. Vague responses (“maybe,” “sure, I guess,” background dialogue) do not authorize the change; an expired proposal requires a new confirmation.

### J-03 — Mobile discussion during a journey

1. Before driving (or with help while parked), user opens the Android app and taps **Start conversation**. No wake word is required.
2. Audio routes over the preferred device/car connection. Agent offers a brief issue summary and can continue with “next,” “why?”, “remind me of the tradeoff,” etc.
3. UI is minimal; the agent avoids long URLs, code blocks, or instructions to inspect a screen while moving.
4. Transient connectivity interruptions trigger a short reconnect announcement where possible. The client keeps conversational review progress for resumption; it never repeats a write blindly after reconnection.
5. Ending the call returns normal audio focus to music/navigation. **Navigation cues and safety-critical car audio must not be overridden by the app.**

### J-04 — Continue from desktop on mobile

1. Desktop session discussed two issues and left a third pending review.
2. Mobile user starts a new session: “Continue where I left off.”
3. The assistant recovers the latest review cursor, the last decisions, and unresolved issues. It **revalidates current GitHub state** before recommending or executing updates.
4. An unconfirmed action from an earlier session does **not** remain executable; it must be proposed and confirmed again.

### J-05 — Quiet processing and thinking sound

1. A read-heavy operation takes longer than a configurable threshold (proposed default: ~1.5 seconds).
2. Agent plays a very low-volume, non-intrusive, optionally muted thinking clip (or gives a short verbal progress cue).
3. It stops the thinking audio as soon as the agent speaks, is interrupted, or the operation ends.
4. For work lasting longer than ~8 seconds, it adds a useful spoken progress update rather than looping indefinite hold music.

## 5. Functional requirements

Priorities: **P0** = required for launch; **P1** = near-term; **P2** = later.

| ID | Priority | Requirement | Acceptance criteria |
|---|---|---|---|
| FR-001 | P0 | Desktop local wake-word detection (“Hey Computer” initially; configurable after model packaging). | Idle audio stays on the workstation; speaking the phrase starts a session without manual interaction. |
| FR-002 | P0 | Desktop inactivity/stop handling. | “Go to sleep” ends the call; configurable idle timeout returns to passive wake mode. |
| FR-003 | P0 | Duplex voice conversation and barge-in. | User can interrupt speech; agent stops its audible response and processes the next turn. |
| FR-004 | P0 | Manual mobile session start/stop. | Android app never opens the mic on its own after process launch; explicit Start and End are visible. |
| FR-005 | P0 | Read-only issue discovery and triage. | Filter by repo, labels/project fields, assignee, dependencies, recent discussion, and blockers; name exact repository and issue number. |
| FR-006 | P0 | Grounded prioritization and explanation. | Answer includes why an issue requires sign-off and what decision is missing; no invented GitHub metadata. |
| FR-007 | P0 | Conversational continuity within a session. | “That issue,” “next,” “compare those,” and “what if” resolve to current review context. |
| FR-008 | P0 | Issue comment creation and configured workflow changes. | Tool updates actual GitHub state only after user confirmation; result is verified with a readback. |
| FR-009 | P0 | Two-stage voice confirmation of mutations. | Spoken confirmation binds one proposal to one target, specific change set, and short expiry; no side effects before confirmation. |
| FR-010 | P0 | Audit trail and operation result. | Store actor/session, target, proposed and confirmed changes, timestamps, provider response, and outcome. |
| FR-011 | P0 | Repository allowlist and GitHub authentication. | Neither listing nor editing crosses configured installations/repositories and authorization scopes. |
| FR-012 | P0 | Failure and ambiguity handling. | On ambiguous commands, expired confirmations, stale issues, or GitHub errors, ask/notify rather than guessing or claiming success. |
| FR-013 | P0 | Android Bluetooth / car-audio compatibility testing. | Speak/listen through supported routing; recovery paths for mic/audio route changes and connectivity drops. |
| FR-014 | P1 | Desktop music ducking and restoration. | Lower configured streams on activation and restore exact original per-stream levels, even on timeout/error. |
| FR-015 | P1 | Echo cancellation and wake-word tuning. | Wake-word tests cover speakers playing music, headphones, quiet and noisy rooms. |
| FR-016 | P1 | Thinking sound and progress cues. | Optional low-volume sound starts for longer work, ends promptly, never masks speech. |
| FR-017 | P1 | Persisted review progress across devices. | “Continue where I left off” resumes issue context, not an unconfirmed mutation. |
| FR-018 | P1 | Configurable triage policy per repository. | Labels, project fields, owner/sign-off rules, and ready-for-dev criteria can vary by repo. |
| FR-019 | P1 | Create requested changes without approving. | User can say “request changes” and specify acceptance criteria; comment/status reflect it. |
| FR-020 | P1 | Lightweight companion UI. | Desktop status (idle/listening/thinking/speaking/error), audio controls, mobile session status and recent audit. |
| FR-021 | P2 | Multirepository dependency graph and review batching. | Identify downstream blockers and order reviews to unblock the most work. |
| FR-022 | P2 | Personalized queue defaults and weekly review summaries. | User can save filters, ranking weights, and preferred cadence. |

## 6. Triage behavior and scoring

The assistant should never confuse “high-priority issue” with “issue requiring my decision.” Default queue: **open, authorized issues plausibly blocked on human sign-off**. Repositories supply their own workflow mapping; the agent does not impose global labels by default.

Proposed shortlist ranking (configurable, not an opaque LLM-only score):

- **30% dependency impact:** number and importance of downstream blocked issues, capped to limit outliers.
- **25% decision readiness:** sufficiently specified to make a decision now (candidate options, scope, acceptance criteria).
- **20% urgency:** target date, active milestone, or committed engineering delivery.
- **15% explicit review markers:** project field, labels, mentions, assignee, or owner sign-off requests.
- **10% age of current blocker:** time waiting for decision, not simply total issue age.

Return the **top three by default**, with a brief reason and requested decision; optionally expand to five or a filtered list. Clearly separate facts fetched from GitHub from the agent's interpretation. If a blocker cannot be substantiated, say so and ask a clarifying question.

## 7. Interaction and UX rules

- **Voice brevity:** start with one-sentence context, decision needed, impact, recommendation (if grounded). Go deeper on demand.
- **Interruptible audio:** agent-generated narrative and thinking sounds stop when user barges in; durable writes continue or cancel according to their transactional stage, never silently.
- **Disambiguation:** if two issues might match “that one,” read repo and issue number, then ask.
- **Confirmation:** say exact target and exact intended effect. Only a clear affirmative utterance in a live authenticated session counts.
- **Thinking sound:** never continuous in idle state; quiet, mutable, cancellable, and not a substitute for status/error messages.
- **Driving-safe design:** no complex onscreen workflow, no prompts to read code or URLs, concise choices, and a “pause this until later” escape hatch. User should initiate the session while parked or before moving; car display integration is out of scope.
- **Music coexistence:** desktop wake listening is local; music ducking only after activation, and restoration must be robust. Headphones should work even when ducking is disabled.

## 8. Non-functional requirements and SLO targets

These are **design targets for validation**, not verified performance claims.

| Area | Initial target / constraint |
|---|---|
| Wake latency | p95 activation within 1.0 second after wake phrase ends in test conditions |
| False positives | Target fewer than one unintended wake per eight hours of representative music/work audio; measure before release |
| False negatives | Under 5% in normal speaking-volume tests at a desk; document louder-speaker limitations |
| Response latency | p50 first audible reply ≤2.0 s, p95 ≤4.0 s on healthy network for simple turns |
| Audio interruptions | Barge-in stops assistant audio in ≤400 ms under test conditions |
| GitHub write correctness | Zero known unconfirmed writes; verify state after every confirmed mutation |
| Write retry safety | No duplicate comments / repeated changes after reconnect or voice interruption |
| Availability | Local wake agent auto-starts with user session and recovers from service failure; backend deployed independently if mobile availability is needed when workstation sleeps |
| Mobile bandwidth | Adaptive voice-only WebRTC; reconnect gracefully and surface connection quality problems |
| Data retention | Default no raw wake audio stored; conversation transcript retention configurable, proposed 30 days; audit history retained per configured policy |
| Accessibility | Muted thinking sound, adjustable speaking speed/volume, optional transcript for later review |

## 9. Permissions, privacy, and trust

- **Wake-word privacy:** capture and classify locally. Do not upload ambient audio or continuously transcribe when idle. Publish a short mic-active indicator and easy kill switch.
- **GitHub:** use a repository-scoped GitHub App or other fine-grained credentials held on the backend, never shipped in the mobile app or model prompt.
- **Authenticated identity:** voice confirmation expresses *intent* but does not prove *identity*. The user's signed-in session authorizes access; stolen/unlocked sessions are a separate threat. Require device unlock / app authentication as policy dictates.
- **Prompt injection:** issue descriptions/comments are untrusted; their contents cannot expand tool permissions or authorize writes.
- **High-risk actions:** v1 permits only configured single-issue transitions/comments/labels. Closing issues, destructive edits, bulk operations, changing permissions, and PR/deployment actions are disabled until separately designed and governed.
- **Audit:** record the approval text as interpreted, normalized proposal, GitHub API result, and identity. Keep raw audio only if user explicitly opts into recording.

## 10. Success measures

- At least **80% of sampled review conversations** end with a clear decision, change request, or deliberately deferred issue.
- At least **90% of the top-three recommendations** in acceptance tests are correctly identified as requiring human sign-off, based on manually labeled ground truth.
- **100% of GitHub mutations** in test runs require a matching confirmation and produce an outcome record.
- No unannounced duplicate writes during disconnect/reconnect tests.
- Satisfactory real-world wake performance while playing music at the user's normal volume (measure and tune).
- A mobile user can start one session, hear a backlog summary, discuss an issue, and end without needing visual interaction after the initial start.

## 11. Release plan

| Phase | Deliverables | Exit criteria |
|---|---|---|
| **0 — Spike** | LiveKit agent; desktop audio loop; Android RN starter; GitHub read-only tool; network benchmark | Audio duplex/interruptions demonstrated; GitHub queue works; desktop/mobile can join distinct sessions |
| **1 — Read-only vertical slice** | Desktop wake word; mobile manual start; issue prioritization; basic UI; speech/agent observability | Spoken top-three backlog summary works from both clients; music coexistence tested |
| **2 — Safe writes (MVP)** | Prepare/confirm/execute; GitHub App permissions; comment/label/status; audit, reconciliation and stale checks | Confirmation and adversarial tests pass; real issue updates verified; no duplicate writes |
| **3 — Polish and mobility** | Audio ducking, thinking sound, reconnection, Bluetooth/car testing, cross-device resumption | Meets defined SLO targets and safety UX checks |
| **4 — Expansion** | Repo-specific templates, dependency ranking improvements, iOS, more workflows | Separately approved priorities |

## 12. Decisions recorded and open product decisions

### Decided

1. Desktop is **Linux-first** with local wake-word activation.
2. Mobile is **React Native / Android-first** with **manual** conversation start; no mobile wake-word requirement.
3. Conversation is two-way, low-latency, interruptible, and largely voice-controlled.
4. First integration is **GitHub issue triage and explicit single-issue updates**.
5. The assistant supports optional audible feedback while working.
6. One shared backend supports both clients, with persisted progress where useful.

### Recommended defaults subject to validation

- Wake phrase: “Hey Computer.”
- Idle conversation timeout: 45 seconds; configurable.
- Queue size: three issues; user can request more.
- Initial action permissions: comment, label, one configured workflow transition.
- Thinking sound: optional and quiet; start after 1.5 s; spoken progress after ~8 s.
- Local wake daemon + backend model/API; no requirement that the large model run on the workstation.

### Open for implementation kickoff

- Which organizations/repositories to enable, and whether GitHub Projects v2 fields are used for readiness.
- Whether backend must be accessible while every workstation is offline; if so, choose a dedicated always-on host or managed media plane.
- Authentication provider and acceptable reauthentication policy for mobile.
- Preferred model/vendor and monthly voice/LLM spend cap.
- Whether user wants optional text transcript storage at all.

## 13. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Background music masks wake phrase or speech | Headset support, AEC using playback reference, noise suppression, volume ducking, threshold calibration, false-positive tests |
| Model invents repository state | Ground all recommendations in GitHub reads, include issue identifiers, separate inference, re-fetch before writes |
| Spoken “yes” applies to the wrong issue | Per-action short-lived proposal ID, explicit issue readback, one active confirmation at a time, abort on context switch |
| GitHub write partially succeeds | Per-step execution log, idempotency, verify each mutation, report partial result and propose safe remediation |
| Issue comments contain malicious instructions | Treat repository text as data; backend validates commands from structured tool calls, not prompt content |
| Unsafe distraction in vehicle | Simple audio interaction, no required in-drive screen use, deferrable complex reviews, initial activation before movement |
| Backend unavailable when workstation sleeps | Split desktop wake listener from an independent, reachable agent host and token service |
| Unexpected audio recording | Local-only idle wake detection, UI microphone indicator, no automatic mobile capture, explicit retention controls |

## 14. Source references

Technology feasibility is based on official vendor documentation; requirements and SLOs above are product proposals.

- LiveKit wake-word detection: https://docs.livekit.io/agents/multimodality/audio/wakeword/
- LiveKit Agents: https://docs.livekit.io/agents/
- LiveKit React Native starter: https://docs.livekit.io/frontends/start/starter-apps/react-native/
- LiveKit background audio: https://docs.livekit.io/agents/multimodality/audio/background-audio/
- LiveKit Node.js tools: https://docs.livekit.io/agents/logic/tools/definition/
- GitHub REST issues: https://docs.github.com/en/rest/issues
- GitHub Apps authorization: https://docs.github.com/en/apps/using-github-apps/authorizing-github-apps
- PipeWire echo cancellation: https://pipewire.pages.freedesktop.org/pipewire/page_module_echo_cancel.html
- Android car app policy (not an Android Auto claim): https://developer.android.com/docs/quality-guidelines/car-app-quality
