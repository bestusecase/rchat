# Android notification repairs, second pass

Status: resolved

## Problem Statement

The previous pass repaired six ways the notification threads could lie. A second
review of it found that two of those repairs rest on claims the code does not
hold, and that a third defect was left in place on the platform side.

The claim that a message read in the app can never resurface is guarded by a test
that passes against the code it was written to replace: it flips the visibility
answer the moment it is asked, which the old code also honoured when it sampled
before parsing. Nothing in the suite pins the actual rule — that the question is
asked at commit time and not for events that can never be posted — so the repair
could be undone by moving one line and every test would stay green.

The second unproven repair is the other half of the same promise: when the
delivery step refuses a snapshot because the user is looking, it forgets what was
accumulated. That logic lives in the Kotlin bridge, past the edge of every
automated test, and the record does not carve it out, so a maintainer reading the
suite would believe it is covered.

The third is a hole in the fail-closed rule itself. The bridge now treats "the
platform could not answer" as visible so that message text is never posted on a
guess — except at the one point where the answer is invented rather than
observed: when the bridge has no context yet, it reports that nobody is looking,
and the accumulator believes it.

The rest is record and vocabulary drift: the README still calls the quantity
unseen, the two shipped specs still say they are waiting to be implemented, and
an allocator's comment promises the lowest free id where the code keeps every id
it has ever handed out.

## Solution

Make the fail-closed rule unconditional: an app that cannot say whether it is on
screen is treated as visible, at the platform edge as well as across the bridge,
so nothing accumulates and no text is posted on a guess.

Bring the delivery step inside the tested state machine. One event in, one
decision out: the delivery step asks the same visibility question, refuses when
the user is back, forgets what it refused, and otherwise hands the snapshot to a
poster. The Kotlin bridge loses its own copy of that rule and becomes what the
design says it is — a renderer.

Make the tests hold the claim: the visibility question is asked only for an
event that could be posted, only at commit time; and a refused delivery leaves
nothing behind, while a message that arrives after the resume is the only thing
the next notification contains.

Finish the rename, mark the shipped specs resolved, and pay down the small
readability debts the review named so that the next reader does not have to
rediscover why the reply to "can the user see the app?" is asked twice, or why
the id map never shrinks.

## User Stories

1. As someone who opens the app just as a message arrives, I want the guarantee
   the last pass promised to be the guarantee the tests hold, so that a message I
   read cannot come back in a later notification.
2. As a maintainer, I want a test that fails against the old pre-parse sampling,
   so that the timing rule is pinned by evidence rather than by a comment.
3. As a maintainer, I want the visibility question never asked for an event that
   cannot be posted, so that the question's position in the sequence is the
   behaviour the suite protects.
4. As a maintainer, I want a delivery that refuses a snapshot to also forget it,
   covered at the same seam as everything else, so that the second half of the
   promise cannot regress behind my back.
5. As someone reading their messages, I want a resume during a burst to leave
   only what arrived after I stopped looking, so that the shade never counts text
   I have already seen.
6. As a maintainer, I want that rule to live in one place, so that changing it
   cannot land in one language and be forgotten in the other.
7. As someone whose app process has just started, I want it to say nothing when
   it cannot yet tell whether I am looking, rather than post on a guess, so that
   message text neither leaks nor flickers onto a screen I am holding.
8. As someone with a phone that has just booted, I want a missing platform
   context to count as visible, so that the fail-closed rule covers the one path
   that used to answer the other way.
9. As a maintainer, I want the renderer to render and nothing else, so that what
   is posted is decided in one testable place.
10. As a maintainer, I want the visibility question asked at commit time and
    asked again at delivery, so that both halves of the resume race have a guard
    rather than one guard and one assumption.
11. As someone who reads the README, I want it to call the quantity unread, so
    that the store, the snapshot, the renderer and the prose share one name.
12. As a reader of the issue tracker, I want the two shipped specs marked
    resolved, so that the record says what is done instead of what is ready.
13. As a maintainer, I want the two suppression inputs bundled into one named
    value, so that the asymmetry between them — muting known at arrival,
    visibility asked at commit — is visible where it is passed.
14. As a maintainer, I want the child-id allocator's comment to say that ids are
    kept for the life of the process, so that nobody reads "the lowest free id"
    as "an id comes back when a conversation goes away".
15. As a maintainer, I want the call site to stop shadowing the allocator's name,
    so that reading the line does not require holding two meanings in mind.
16. As a maintainer, I want the two pieces of notification bookkeeping cleared
    through one helper, so that a future third piece cannot be forgotten in one
    of the two clear paths.
17. As a maintainer, I want one builder for the test event instead of four
    near-identical literals, so that changing the event shape is one edit.
18. As someone with several conversations waiting, I want the summary count, the
    child counts, the five-line cap and the child ids unchanged by this pass, so
    that what I see is the same or better and never a regression.
19. As someone whose phone is locked on a desk, I want the count-only public
    version to stay count-only, so that a bystander learns nothing from these
    repairs.
20. As someone receiving a burst, I want no duplicate notification and no
    reshuffle while this lands, so that the shade stays stable.
21. As someone who mutes notifications, I want muting to remain the first gate,
    so that nothing is accumulated before the visibility question is asked.
22. As a maintainer, I want the "could not answer" path logged, so that a device
    pass can tell a correctly suppressed post from a broken bridge.
23. As a maintainer, I want a test proving the forget cannot discard a message
    that arrived after the resume, so that the review's open question is answered
    by evidence rather than by argument.
24. As someone holding the phone, I want the device pass repeated for the parts
    no automated test can see, so that the badge, the ids and the lock-screen
    posture are confirmed by eye once more.

## Implementation Decisions

**Fail closed at the platform edge.** The bridge reports visible when it has no
context to reason about, so "cannot answer" and "not looking" stop being the same
answer. The existing rule across the bridge is unchanged: off Android nothing is
posted and nothing accumulates; a JNI refusal or exception is logged and treated
as visible. The three states are now consistent — visible, and therefore silent.

**One delivery step.** The event loop's body becomes a single function that owns
an event from arrival to shade: it locks the accumulated threads, calls the
accumulator, and if a snapshot comes back, asks the visibility question once
more. If the user is back, the snapshot is refused and the accumulator is
forgotten; otherwise the snapshot goes to a poster supplied by the caller. In
production the poster is the JNI call; in tests it is a closure that records what
it was given. The step returns whether anything was posted, which is all the
loop needs.

**Why the question is asked twice.** The commit-time sample is what stops a
message being accumulated; the delivery-time sample is the backstop for a resume
that lands between the commit and the post, and forgetting there is what stops
the next background message from posting text already read. Asking twice is
deliberate and is documented at both sites, since a reader who sees two questions
and no explanation will delete one of them.

**The renderer stops deciding.** The Kotlin bridge no longer consults visibility
before posting and no longer forgets on refusal: both moved into the delivery
step. Its null-context early return stays, because posting nothing is the
fail-closed answer, but it is no longer a policy decision. The native entry point
that lets the bridge forget the accumulator remains for the foreground clear and
the engine stop, which are its real callers.

**The over-forget question is answered, not coded around.** Review asked whether
an unconditional forget could discard a message that legitimately arrived after
the resume. It cannot: events are consumed by a single task, so there are never
two appends in flight, and once the activity flips its flag every append declines
at commit time. At the moment of refusal the accumulator can therefore only hold
messages that arrived before the resume — exactly the ones the user may already
have seen. No conditional forget is added; a test pins the sequence instead, so
the reasoning is checked rather than remembered.

**What the tests may observe.** The visibility question is part of the seam's
behaviour, not an implementation detail: how often and for which events it is
asked is a testable claim, because that ordering is the whole repair. This is
deliberately narrower than asserting on platform call counts, which stays out of
bounds. The poster is likewise a collaborator supplied by the test, and what the
test asserts is the shade it would have produced.

**A named suppression value.** The accumulator stops taking a bare boolean and a
bare closure in sequence. It takes one small value whose name says what it is —
who may be told — with its documentation recording that muting is sampled when
the event arrives and visibility is asked when the append commits. Both call
sites construct it the same way.

**Bookkeeping.** The posted child ids and the conversation-to-id allocations are
cleared by one private helper, called from both the foreground clear and the
cancel-everything path, so the two cannot drift. The allocator's name stops
shadowing itself at the call site, and its comment states that an allocated id is
kept for the life of the process and never released — the policy the design
already chose, now written down honestly.

**Unchanged.** The eight-message bound, the five-line cap by snapshot position,
the summary's unread total, each child's own count, the public count-only
version, the private body, the group, the channel and its importance,
tap-to-open, the permission request, the foreground-service notification, the
mute predicate's priority over accumulation, and the id allocation policy. The
message channel's own rendering code is not edited beyond the bookkeeping helper.

**Documentation.** The README's remaining "unseen" becomes "unread", matching
every other surface. Once this pass ships, both earlier specs move from
`ready-for-agent` to `resolved` per the tracker's status convention, and this
spec records its own revisions under its comments rather than rewriting the
earlier records.

## Testing Decisions

A good test here describes observable behaviour: an event and an answer to "can
the user see the app" go in, and what the shade would receive comes out — which
conversations appear, what they contain, what is suppressed, and whether the
question was asked at all. Tests do not assert on internal maps, on platform call
counts, or on the order in which the summary and children are posted.

**Two seams, one module.** The accumulator (`observe`: event in, snapshot out)
and the delivery step (event in, shade out) both live in the mobile shell's
notification module, which already holds this state machine and its tests. The
delivery step is the higher seam and is the preferred place to prove behaviour;
the accumulator is tested directly only for the properties that are about the
append itself.

New coverage:

- A muted event, an unrelated event, a malformed event and the app's own message
  each leave the accumulator empty *and* are decided without asking whether the
  user is looking — the assertion is that the question was never asked.
- A message whose visibility answer is "visible" at commit time produces no
  snapshot, and one committable message asks the question exactly once at the
  accumulator seam.
- A delivery whose visibility answer flips between the commit and the post posts
  nothing, forgets what it committed, and leaves the next background message
  alone in the following notification.
- A delivery whose answer stays "not looking" posts the snapshot and keeps the
  accumulation, so the following event's notification still contains both
  messages.
- The earlier resume tests keep their behaviour, including the case where the
  append wins the lock and the following clear discards it.

The existing suite — twenty-one tests — keeps passing with only the vocabulary
and helper names moving. Prior art is the same module's `Case` harness, the
event-JSON-in-decision-out style, and the cell-backed visibility provider the
previous pass introduced.

**Deliberately not covered by automated tests.** The Kotlin null-context default,
the rendering, the child-id allocation, the summary number and badge, and the
public-version mirroring. There is no Kotlin test harness and this pass does not
add one; the platform surface is declarative and is checked once, by eye.

**Device acceptance pass.** On the Pixel 8: with the app backgrounded, confirm
the summary's number still equals the unread total and the launcher badge agrees;
a second peer still adds a child rather than replacing one; several waiting
conversations still render five summary lines; and opening the app during a burst
still leaves only messages that arrived after the resume, with no stale text in
the shade. Send one message at a time, because the platform's enqueue rate limit
can shed a burst and muddy the result.

## Out of Scope

- Releasing child ids when a conversation is cancelled, or any other change to
  the allocation policy.
- Deep-linking a child to its conversation.
- A user preference to hide content, per-conversation muting, and notification
  actions.
- Changing the eight-message bound, the summary wording, the channel or its
  importance, the tap behaviour, or the permission request.
- A Kotlin or Robolectric test harness.
- Message previews on desktop, or any change to desktop or browser behaviour.
- The concurrent mesh and network work elsewhere in the tree.

## Further Notes

This repairs two commits, not the working tree: the tree carries another
session's mesh feature that does not compile. Build and test from a throwaway
checkout of the notification work plus this patch, as the previous pass did, and
keep that tree's unrelated uncommitted lines out of any commit — in particular
the working copy's added field in the test helper, which does not exist in the
committed interface.

The review that produced this pass reported two axes separately: the standards
axis named the fail-closed gap, the shadowed allocator, the duplicated clear
paths and the comment drift; the spec axis named the unguarded race test, the
untested refusal and the README vocabulary. Nothing was found outside those two
lists, and no behaviour beyond them is being changed here.

The previous pass's device evidence remains the reference for what the shade
should look like; this pass adds no visual change, so a difference on the device
would mean a regression, not a new behaviour.

## Comments

**2026-11-16 — implementation notes.** Implemented as `d09f0a4` (rebased from
`4a478f0`, which was the same diff on `6a29ce3`) in a throwaway worktree. The
accumulator and the delivery step both live in
`apps/mobile/src-tauri/src/notify.rs`; the Kotlin bridge is a renderer again.

- The named suppression value is `Audience`, carrying `muted` (sampled when the
  event arrives) and `visible` (asked when the append commits and again at
  delivery). The event loop constructs one per event.
- One module-level `deliver` locks the accumulated threads, delegates to
  `Threads::deliver`, and hands the snapshot to the poster supplied by the
  caller: the JNI call in production, a recording closure in tests. It returns
  whether anything was posted.
- The ordering test was checked against the old pre-parse sample and failed
  there with four visibility questions, one per unpostable event, so the rule
  is pinned by evidence rather than by a comment.
- `cargo test --lib notify`: 26 passed (21 existing, 5 new). The run used a
  throwaway checkout at `6a29ce3` plus this patch; after rebasing onto
  `0137821`, the parent needs the other session's in-flight `legacy_network`
  line in the mobile `lib.rs` to compile, and with that applied temporarily the
  same 26 pass, alongside the two attachment tests in the working copy. Clippy
  and `cargo fmt --check` are clean, `:app:compileUniversalDebugKotlin` passes,
  and the universal debug APK builds.
- The bridge answers visible and logs when it has no context yet, so "cannot
  answer" is both fail-closed and visible in logcat; `postedChildren` and
  `childIds` are cleared through one `forgetThreads` helper.
- The device acceptance pass was only partial: the APK installed and launched on
  the Pixel 8 without a crash, but the message-driven checks (badge, child ids,
  five-line summary, lock-screen posture) were not repeated because no live peer
  path was available in this session. Those still need a human with two peers.
