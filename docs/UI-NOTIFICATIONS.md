# Desktop notifications

## Reading and dismissing

Errors and warnings stay visible by default. Success and information messages expire
at four seconds. Every toast has a localized, keyboard-accessible dismiss button.
Errors announce as atomic, assertive alerts; other severities use atomic, polite
status announcements. Notifications do not steal focus.

An explicitly supplied duration still wins, including for errors and warnings.
Pointer presence or keyboard focus inside a timed toast pauses its clock. It resumes
only after both have left, using the remaining time, not a fresh duration. Moving
focus between descendants does not resume it. Zero and negative durations retain
asynchronous immediate expiry; they do **not** mean persistent.

## Bounded queue tradeoff

At most five notifications are retained, in arrival order. On overflow, the oldest
message at the lowest severity is evicted: success/info < warning < error. An incoming
lower-severity message is dropped if all five retained messages outrank it. Thus six
successes cannot erase an existing error or warning. Five errors can suppress new
successes/warnings; a sixth error replaces the oldest error. Persistence is therefore
not an unlimited history guarantee. Dismissed and evicted timed messages release
their timers; dropped messages never allocate one. No unbounded backlog is kept.

## Layout and compatibility evidence

The stack is width- and height-constrained to the viewport with vertical scrolling.
Unbroken paths/diagnostics wrap inside a shrinkable text column. Toasts no longer use
the horizontal slide-in animation; unrelated animation definitions are unchanged.

The pre-implementation store/component source matched `e85417c` (no source diff
against HEAD). The same focused tests observed nine compatibility passes before and
after implementation: success/info four-second expiry, explicit durations for all
four severities, zero/negative expiry, and latest-five equal-priority ordering.
Intentional differences are persistent defaults, severity-preserving overflow,
timer cancellation, pause/resume handlers, announcements and viewport classes.
Read-only TypeScript callsite inspection found 107 production `addToast` calls: only
two pass a duration (both 8000ms, in InjectModal); neither uses zero or negatives.
The `add(type, message, duration?)` and `addToast` void APIs remain unchanged.

Tests exercise the public store with Node fake timers and authored JSX handlers via
the existing registerHooks harness. They do not establish browser layout, screen-reader
announcements, native focus behavior or human readability acceptance. The parent owns
full-suite/build integration and candidate-bound runtime verification.
