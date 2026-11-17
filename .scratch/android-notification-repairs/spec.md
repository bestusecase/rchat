# Android notification repairs

Status: resolved

## Problem Statement

The per-conversation notification threads work, but a two-axis review of the
change that shipped them found six defects, two of which the person holding the
phone can feel.

The serious one is a timing hole in the promise that a message read in the app
never resurfaces. The accumulator asks whether the user can see the app *before*
it takes its lock and appends. If the app resumes in that window — the user
taps the icon at the moment a message lands — the resume's clear finishes first,
the append then lands on an empty accumulator, and the post that follows is
refused because the app is now visible. The message stays accumulated. The next
message, arriving after the user has pocketed the phone again, posts a thread
containing text they have already read. That is exactly user story 14 of the
original spec, failing.

The second visible defect is the summary's number. The count-only notification
it replaced carried the unread total as its number, which is what drives the
launcher badge; the summary now carries none, so the badge count disappeared
while the channel still asks to show a badge.

The rest are quieter: two conversation ids that hash alike overwrite each
other's child, a message whose event carries no timestamp is treated as the
oldest in its thread and evicted first while still counted, the summary's
five-line cap is keyed off the wrong set and can emit a sixth line, and the
recorded design now contradicts the code about ordering — the spec still claims
arrival order is timestamp order with no sorting, which the device pass
disproved.

## Solution

Close the visibility hole at its source: the accumulator samples visibility at
the moment it decides to append, while holding the lock that the foreground
clear also needs, so the two can no longer interleave into a resurrection. A
snapshot the post step refuses because the user is looking is also forgotten, so
a stale snapshot cannot seed the next post.

Give the summary its number back: the total unread count, matching the count the
channel badges. Children keep their own conversation's unread count, and every
public version mirrors the number of the notification it stands in for.

Make a child's notification id unique for the process rather than a 30-bit hash
of the conversation id, so no conversation can silently replace another, and
count summary lines by conversation instead of by id.

Reject an event whose message has no integer timestamp as malformed, the same
way an unrelated event is already rejected, so a missing clock can never sort a
message to the head of its thread.

Correct the record: the ordering rule is the sender's timestamp with arrival
order breaking ties — which is what the conversation view shows — and the
original spec says so, with the revision noted under its comments. Use one name,
unread, for the quantity the notifications count.

## User Stories

1. As someone who opens the app just as a message arrives, I want that message
   never to appear in a later notification, so that the shade only tells me
   about things I have not already read.
2. As someone who glanced at the app and then pocketed the phone, I want the
   next notification to contain only messages that arrived after I stopped
   looking, so that the count is not inflated by text I have seen.
3. As someone glancing at the shade, I want the summary card to carry the total
   number of waiting messages, so that the launcher badge and the summary agree.
4. As someone with several active conversations, I want each child to carry its
   own conversation's unread count, so that the child and the summary do not
   disagree.
5. As someone with the device set to hide sensitive content, I want the
   count-only public version of every notification to carry the same number as
   the private one, so that the locked screen and the unlocked shade agree.
6. As someone with two active conversations, I want neither conversation to
   overwrite the other's notification, so that no peer silently disappears from
   the shade.
7. As someone with several conversations, I want the summary to list exactly one
   line per conversation, up to five, so that the list never grows past what the
   platform renders.
8. As someone receiving a message whose event carries no timestamp, I want it
   never to masquerade as the oldest message in the thread, so that trimming
   does not discard it first while the count still includes it.
9. As someone receiving a burst, I want messages ordered by the timestamp the
   sender put on them, with arrival order breaking ties, so that the thread
   reads in the order they were sent.
10. As someone whose two messages were sent in the same millisecond, I want them
    to keep the order they arrived in, so that the thread does not reshuffle
    itself between posts.
11. As a maintainer, I want the recorded design and the code to agree about
    ordering, so that the next person does not implement the opposite of what
    the spec says.
12. As a maintainer, I want one name — unread — for the quantity the
    notifications count, so that the store, the snapshot and the renderer do not
    drift into synonyms.
13. As a maintainer, I want the visibility predicate to fail closed when the
    bridge cannot answer, so that message text is never posted on a guess.
14. As a maintainer, I want the post step's refusal to clear the accumulator it
    refused, so that a stale snapshot cannot seed the next notification.
15. As a maintainer, I want the bound's rationale to be honest about what it
    caps, so that nobody believes eight messages is everything the expanded
    shade can render.
16. As a maintainer, I want the lifecycle helper's comment to name what
    actually calls it, so that a sticky restart's behaviour is documented
    truthfully.
17. As a reader of the repository, I want the README's ordering and badge
    statements to be true of the code, so that it describes the product
    truthfully.
18. As a reader of the issue tracker, I want the original spec to record these
    revisions under its comments, so that the design history is legible.
19. As someone with the phone locked on a desk, I want the lock screen to keep
    showing only a count, so that these repairs do not widen what a bystander
    can learn.
20. As someone using a group conversation, I want the title and per-author
    attribution to survive the repairs, so that the thread still makes sense.
21. As someone who denies the notification permission, I want the app to keep
    working silently, so that the refusal is still not fatal.
22. As a maintainer, I want every repair covered at the existing state-machine
    seam, so that none of them needs a device to regress unnoticed.
23. As a maintainer, I want the device pass repeated for the badge and the
    summary lines, so that the two changes the tests cannot see are confirmed by
    eye.

## Implementation Decisions

**Visibility at commit time.** The accumulator stops receiving a visibility
sample taken by the caller. It receives a visibility provider and queries it
while holding its own lock, immediately before deciding to append. The
foreground path flips the activity-resumed flag *before* it clears the
accumulator, which it already does; together these make both interleavings
safe. If the clear wins the lock, the following append samples a visible app and
declines. If the append wins the lock, the clear that follows discards what it
appended. Nothing that arrived before the resume can survive into a snapshot
posted after it.

**A refused snapshot is forgotten.** The post step already declines when the
user can see the app. It must also forget the accumulator at that moment: the
snapshot it refused came from state taken while the user was away, so keeping it
would let the next background message post it. This is the second half of the
same promise and costs one bridge call.

**Summary number.** The summary carries the total unread count as its number;
each child carries its conversation's unread count. Public versions carry the
same number as the notification they stand in for. This restores the badge count
the count-only notification had and resolves the original spec's "both styles
carry ... as their number" in favour of the total for the summary, since a
summary spans conversations and has no single conversation's count to carry.

**Unread, not unseen.** The quantity is called unread everywhere — accumulator,
snapshot fields, renderer, comments — matching the store's existing vocabulary.
The snapshot field names change with both sides in the same change; nothing
outside the notification path reads them.

**Timestamps.** An event whose message carries no integer timestamp is
malformed and ignored, consistent with the rule that malformed or unrelated
events are ignored. The accumulator never invents zero or the local clock for a
message it is about to bound and order.

**Child ids.** Child ids are allocated, not hashed. The renderer keeps a map
from conversation id to notification id for the life of the process, assigning
the lowest free id at or above the reserved base the first time it sees a
conversation; the map is cleared with the posted set whenever the shade is
cleared. This makes ids unique by construction, so no conversation can overwrite
another. Stability across process restarts is not required: the service cancels
every notification before it promotes itself, so a new process never inherits a
child. The five-line cap counts conversations by their position in the snapshot,
not by the size of the posted-id set, so a cap can never be defeated by a
collision.

**Bound rationale.** Eight stays, because the original spec chooses it. The
comment changes: eight caps shade bloat and memory, it is not a claim that the
expanded shade renders no more. Trimming can therefore hide text the platform
could have displayed, and the count remains the truthful part.

**Visibility failure.** The bridge distinguishes "asked and the answer is no"
from "the bridge could not answer". Off Android the predicate is reported as
visible, so nothing accumulates where nothing is posted. On Android a bridge
failure is logged and treated as visible, so content is never posted on a guess;
the next event re-evaluates.

**Lifecycle comment.** The clear helper is called when the app returns, when the
engine stops and when the service is destroyed — not while it starts, which
calls the cancel-everything path. Its comment says so.

**Documentation.** The original spec's ordering paragraph is corrected to the
sender timestamp with arrival-order tie-break, and the summary-number sentence is
resolved as above; both revisions are recorded under that spec's comments, per
the tracker's convention. The README's existing ordering and badge statements
are kept true by the code rather than softened. The engine-stop cancellation
added during the original work is accepted and recorded; it is consistent with
the story that a stale notification must never outlive the process that can
explain it.

**Unchanged.** The message channel and its importance, tap-to-open behaviour,
the permission request, the foreground-service notification, the eight-message
bound, private visibility with a count-only public version, and the mute
predicate's priority over accumulation.

**No desktop change.** The repairs live in the Android notification path. The
desktop and browser clients keep their count-and-chime behaviour.

## Testing Decisions

A good test still describes observable behaviour: an event and a visibility
state go in, and the snapshot the user would receive comes out — which
conversations appear, what order their messages sit in, what the numbers say,
and what is suppressed. Tests do not assert on internal maps, on JNI call
counts, or on the order in which the summary and children are posted.

**One seam, the existing one.** Every repair that can be decided in the
accumulator is tested there: the state machine in the mobile shell's Rust code,
taking an event, a visibility provider and the store, returning a snapshot. The
seam gains no new collaborators beyond the provider, which tests supply as a
small closure they can flip, so the interleaving that caused the race is
reproducible without threads or a device.

New coverage at that seam:

- A visibility sample that flips to visible after the event is read but before
  the append is committed leaves nothing in the snapshot.
- An append that commits before the resume, followed by a clear, leaves nothing
  either.
- Equal timestamps keep arrival order, and that order survives trimming.
- An event without an integer timestamp is ignored entirely.
- The snapshot's total unread equals the sum of the conversations' unread
  counts, and each conversation's number is its own count, not the total.
- The existing sixteen tests keep passing unchanged in behaviour, with the
  vocabulary rename applied.

**Deliberately not covered by automated tests.** Child-id allocation, the
summary's number and badge, and the public-version mirroring are Android API
calls whose only proof is the shade. There is still no Kotlin test harness and
this spec does not add one.

**Device acceptance pass.** On the Pixel 8, background the app and check that
the summary card shows the unread total as its number and the launcher badge
count matches; that a second peer's child appears rather than replacing the
first; that the summary never lists more than five lines with several
conversations waiting; and that opening the app while a message is in flight
leaves that message out of the next background notification.

## Out of Scope

- Tapping a notification to open the conversation that caused it.
- A user preference to hide message content from notifications.
- Per-conversation muting.
- Changing the eight-message bound, the channel, the tap behaviour, the
  permission request or the foreground-service notification.
- A Kotlin or Robolectric test harness.
- Message previews on desktop, and any change to desktop or browser behaviour.
- The concurrent mesh and network work elsewhere in the tree.

## Further Notes

This repairs the change committed as `ea777e6`, not the uncommitted work in the
tree. That work is another session's mesh feature and does not compile; build
and test from a throwaway worktree at that commit plus only this patch, exactly
as the original change was validated.

The original spec remains the design record for the feature. This one records
only what changed and why, so the next reader should start there and follow the
comments for the revisions.

The device used for the original pass had no secure lock screen configured, so
the private/public split was confirmed in `dumpsys notification` rather than by
eye. That gap is unchanged by these repairs and should be closed on a device
with a lock screen when one is available.
