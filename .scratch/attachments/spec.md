# Attachments

Status: blocked (device acceptance; see issues/08)

## Problem Statement

The plus button next to the message field is disabled, and rchat carries text
only. A conversation is therefore a poor substitute for the thing people
actually want to send: a photo taken a minute ago, a PDF being discussed, a
screenshot of an error. Every one of those has to leave rchat and go through some
other app, which breaks the promise that the mesh is the whole conversation.

The engine and the wire format are both closer to supporting this than the
interface suggests: the LXMF stack rchat already ships defines standard fields
for images and file attachments, and Sideband already uses them. rchat simply
throws those fields away on receipt and never sets them on send. Meanwhile the
engine has no place to put a file, no way to show one, and no policy for how much
of a mesh link a photo is allowed to consume — the last of which matters,
because the same feature that is trivial over LAN is hours of airtime over LoRa.

## Solution

The plus opens the platform's own picker: directly on desktop, through a
two-choice sheet on Android so that a photo can be taken with the camera rather
than hunted for in the gallery. Chosen files appear as chips above the message
field, with the message field itself serving as the caption. Sending is always
explicit.

Images arrive in the thread as images, sized so a photo is legible without
swallowing the viewport, and open into a full-screen viewer on tap. Several
images in one message stack into a small grid. Anything that is not an image —
a PDF, an archive, a voice memo someone sent from another client — arrives as a
file card carrying its name, type and size, with one action: save it.

Attachments travel inside the LXMF message in the standard image and file
fields, so a Sideband user sees our photos and we see theirs. Because they are
part of the message, they inherit everything the message already has: delivery
receipts, the offline queue, and the same read path. The cost is that a message
is now as heavy as its attachment, so the engine refuses anything that is
unreasonable for the link a peer is actually reachable over, and says why.

## User Stories

1. As someone with a photo to share, I want the plus next to the message field to
   open a file picker, so that attaching is one click and not a hunt through
   settings.
2. As someone on a phone, I want the plus to offer the camera as well as the
   gallery, so that I can send the thing in front of me without opening Photos
   first.
3. As someone attaching several files, I want to select them in one go, so that a
   burst of screenshots is one message rather than four.
4. As someone who picked the wrong file, I want to remove it before sending, so
   that nothing leaves the device by accident.
5. As someone attaching a file, I want to type a caption in the normal message
   field, so that the attachment and its explanation are one message.
6. As someone sending an attachment with no caption, I want to send it anyway, so
   that a photo that speaks for itself needs no words.
7. As someone sending a large file over a slow link, I want to be told before
   anything is queued, so that I do not spend an hour of airtime discovering it
   failed.
8. As someone sending to a peer reachable over a radio link, I want a much lower
   limit with a clear reason, so that the mesh stays usable for everyone on it.
9. As someone sending to a peer whose path is not known yet, I want a cautious
   limit, so that an unknown route is not treated as a fast one.
10. As someone sending, I want to see the message appear immediately with its
    attachment, so that the thread reflects what I did rather than waiting on the
    network.
11. As someone whose send fails, I want the attachment to stay on screen with the
    error, so that I can retry without picking the file again.
12. As someone offline, I want my attachment to queue like any other message and
    go out when the mesh returns, so that I do not have to remember to resend it.
13. As someone receiving a photo, I want it displayed in the conversation, so
    that I do not have to open anything to see it.
14. As someone receiving a photo, I want it big enough to make out, so that the
    thread is readable without tapping through.
15. As someone receiving a photo, I want it not to take the whole screen, so that
    the conversation still reads as a conversation.
16. As someone receiving several photos at once, I want them arranged in a small
    grid, so that a burst does not turn the thread into a slideshow.
17. As someone wanting a closer look, I want to tap a photo for a full-screen
    view, so that I can see detail the thread cannot show.
18. As someone in the viewer, I want to save the photo, so that I can keep it
    outside the app.
19. As someone receiving a non-image file, I want its name, type and size without
    opening it, so that I can decide whether I care.
20. As someone receiving a file, I want one obvious way to save it, so that I can
    get it into the app that can actually open it.
21. As someone receiving a message whose attachment exceeds the limit, I want to
    still see the message and be told the attachment did not arrive, so that the
    conversation is not silently missing half of it.
22. As someone whose storage is full, I want old attachments dropped rather than
    the app failing to write, so that chat keeps working.
23. As someone whose old attachment was dropped, I want the bubble to say so
    instead of showing a broken image, so that I understand what happened.
24. As someone who sends a lot, I want my own sent attachments to survive eviction
    as long as possible, so that my library is not the first thing sacrificed.
25. As someone with a peer on Sideband, I want to receive their photos, so that
    the mesh is not partitioned by client.
26. As someone on Sideband talking to an rchat user, I want my attachments to
    arrive, so that the two clients interoperate in both directions.
27. As someone searching, I want to find messages by their caption, so that search
    still works for the messages that have text.
28. As someone glancing at the conversation list, I want an attachment-only
    message to show something meaningful instead of a blank line, so that the
    list stays informative.
29. As someone with notifications on, I want an attachment's notification to say
    what kind of thing arrived, so that the shade is useful at a glance.
30. As someone whose phone is locked, I want the notification to reveal no image
    and no filename, so that a bystander learns nothing.
31. As someone using a screen reader, I want an image message to be announced as
    an image with its caption, so that the thread is not a run of empty bubbles.
32. As someone whose attachment bytes are gone, I want the message, its time and
    its receipt state untouched, so that history is not corrupted by storage
    pressure.
33. As someone using the app in a browser during development, I want attaching to
    work there too, so that I do not need the packaged shell to test it.
34. As a maintainer, I want one code path that ingests both sent and received
    attachment bytes, so that hashing, sniffing, thumbnailing and limits cannot
    drift between directions.
35. As a maintainer, I want the LXMF field shapes pinned by a test that needs no
    peer, so that a format mistake fails in milliseconds rather than on a device.
36. As a maintainer, I want the size, eviction and placeholder decisions covered
    by fast tests that need no network, so that policy does not regress quietly.
37. As a reader of the repository, I want the documentation to stop listing
    attachments as unbuilt, so that it describes the product truthfully.

## Implementation Decisions

**Wire format.** Attachments ride inside the LXMF message in the standard image
field (one `(name, bytes)` tuple) and file-attachments field (a list of them).
rchat sets them on send and decodes them on receipt. No custom field, no
out-of-band transfer, no second protocol: a Sideband peer is a first-class peer
in both directions.

**Byte handover.** The engine and the bridge share a JSON line channel, which is
no place for megabytes. On send, the engine already owns the bytes on disk, so it
passes the bridge a list of `{name, path}` and the bridge reads them to build the
fields. On receipt, the bridge writes each payload into a staging directory
inside the data directory and reports paths in its message event; the engine
moves them into the store, then deletes the staging copy. Both directions are
unbounded in count but bounded in bytes by the policy below.

**One ingest path.** Hashing, MIME sniffing, thumbnailing, row insertion and
eviction live in a single engine routine used by outgoing uploads and incoming
events alike.

**Size policy.** A message's attachment bytes are capped by the peer's bandwidth
class — the coarse `lan` / `relay` / `radio` / `unknown` labels the engine already
derives from path bitrate:

| Peer class | Maximum total attachment bytes |
| --- | --- |
| `lan`, `relay` | 8 MB |
| `unknown` | 1 MB |
| `radio` | 64 KB |

At most four attachments per message. The cap is on the sum, not per file. The
engine refuses an over-limit send with a message naming the class and the actual
size, and creates no message row. Inbound events are capped too: an oversized
attachment is recorded as a row with no bytes and a reason, so the bubble can say
the attachment did not arrive while the message itself is preserved. The ceiling
is a constant with a process-level override so tests can force eviction without
writing half a gigabyte.

**Storage.** A new attachments table, one row per file: owning message, ordinal,
filename, MIME type, byte count, content hash, pixel dimensions, thumbnail hash
and a nullable "missing" reason. Blobs are content-addressed under a directory in
the data directory, deduplicated by hash, with the filename never used as a path
component. Images also get one 480 px thumbnail at ingest. `kind` on a message
becomes `image` or `file` when it carries attachments; the body remains the
caption.

**Ceiling and eviction.** Attachments are capped at 500 MB in total, enforced by
eviction in two passes: oldest received blobs first, and only if that is not
enough, oldest sent blobs. An evicted attachment keeps its row with a reason and
its bubble reads "Attachment removed to save space"; the message text, timestamps
and receipt state are never touched. A startup sweep removes blobs no row
references, which covers crashes between writing and committing.

**Client API.** Upload and download are plain HTTP routes on the existing
loopback server, authenticated with the same token that already guards the socket.
Upload takes raw bytes with a content type and a filename header, and answers with
the hash, size and dimensions. Download serves the original, or the thumbnail on
request, with immutable cache headers. The send command grows an ordered list of
hashes; the engine rejects a hash it has no row for. Binary WebSocket frames stay
unused — they remain reserved for media, not for transfers. The server's request
body limit is raised to comfortably exceed the largest permitted attachment.

**Local rendering, not transcoding.** What is sent is the file as chosen. The
thumbnail is a local display cache and is never substituted for the payload,
because the ecosystem format is a filename plus raw bytes and a transcoded variant
would be a different file than the one the sender chose.

**Interface.** The plus opens the system picker on desktop and a two-item sheet on
Android — "Camera" and "Photo or file" — because Android's document picker does
not reach the camera. Both platforms use a plain file input, which the Android
WebView host and WebKitGTK already route to the native picker, so no dialog plugin
is needed for choosing files. The camera capture is written so that a later QR
scanner reuses the same plumbing.

Selected files sit as chips above the composer; upload happens at send time, so a
removal costs nothing and a failed upload leaves the chips in place with an error
and no message row. Paste and drag-and-drop add to the same chips on desktop.

Rendering: a single image is capped at 320 px tall with its aspect preserved and
no padding, a caption sits under it in the same bubble, two or more images become
a two-column grid of 120 px crops with an overflow overlay beyond four, and a tap
opens a full-screen viewer with a save action. Non-images are cards — glyph, name,
size, one save action — not bubbles.

Saving is a shell concern on both platforms: desktop copies the blob to a path the
user chooses, Android writes it through the system media store. Neither requires
the blob to travel through the interface as data.

**Text-only surfaces.** Conversation-list preview and the Android notification
thread line read the attachment kind or, for a single file, the filename. The
lock screen continues to show the count only, and notifications carry no image.
Search indexes captions only; an attachment's filename is not searchable.

**Capabilities.** The identity payload advertises that this client supports
attachments. Peer capabilities remain unlearned — LXMF as used here carries no
capability exchange — so nothing in the interface is gated on a peer's flag, and
an attachment sent to a client that cannot render it is that client's problem.

**Unchanged.** The offline queue, delivery receipts, message state machine,
event feed, and the notification visibility rules.

## Testing Decisions

A good test states what the user would observe: given a send or an event, what
rows exist, what bytes are on disk, what the interface is told, and what error the
user sees. Tests must not assert on internal maps, on the order in which blobs
were written, or on helper function boundaries.

**One behaviour seam.** The existing engine harness that drives the real engine
against a scripted bridge over a socket is the highest seam available and already
the prior art for this kind of assertion. It carries every behavioural decision
here: the send command the bridge receives, the rows the engine writes for an
inbound event, refusal over the class limits, the placeholder for an oversized
inbound attachment, deduplication, thumbnail dimensions, eviction ordering under a
forced budget, and the orphan sweep. Extending it costs nothing new.

**One format seam, unavoidable.** The bridge is Python and has no harness, and the
one mistake that no Rust test can catch is a field shape LXMF refuses to serialise.
A small script run under the bundled runtime packs and unpacks a message carrying
both field types and asserts the tuples survive. It needs no peer, no network and
no new test framework.

**Deliberately not automated.** Picker behaviour, the camera intent, the shape of
the image grid, the viewer, and saving through the media store. These are platform
declarations and visual layout; the repo has no device test harness and adding one
to assert we called the platform with our own arguments would be all
implementation detail.

**End-to-end smoke.** The two-peer script gains an attachment leg: one engine
sends a small image with a caption over a real Reticulum TCP link, and the
receiving store is asserted to hold the message, the row, the matching hash and a
thumbnail. This is the only test that exercises bridge through LXMF through
Reticulum and back.

**Device acceptance.** On the Pixel 8 against a desktop daemon on the same LAN:
attach from the camera, attach from the gallery, attach several at once, remove
one before sending, send with and without a caption, receive an image and open the
viewer, save a PDF, watch an over-limit send be refused with a legible reason, and
force the budget down to watch an old attachment turn into "removed to save
space".

## Out of Scope

- Searching by attachment filename. Deferred deliberately; captions are searchable.
- Reading QR codes, and any camera use other than taking a photo.
- Playing video or audio inline. They are file cards in v1.
- Thumbnails or attachments in notifications.
- A per-peer or per-conversation setting for accepting or blocking attachments.
- Group conversations, whose fan-out would multiply the cost of one attachment by
  the number of members.
- Resuming, retrying or repairing a partially transferred attachment. A message is
  whole or it is not there.
- Attachment encryption beyond what Reticulum provides, and per-file passwords.
- Editing or replacing an attachment after send.
- Sideband's richer LXMF fields — reactions, replies, comments, telemetry — beyond
  the two attachment fields.

## Further Notes

The repository currently documents the opposite of this design in one place —
attachments are listed under "Not built yet" — and the plus button's tooltip
promises a later update. Both need correcting in the same change.

The disabled plus button and the `attachments` capability flag were both placed
in anticipation of this feature; the flag was never populated and the button never
wired. Neither needs redesign, only connection.

Bandwidth class already exists per peer and is refreshed from the path table, which
is why the size policy needs no new measurement — only a consumer.

The 8 MB ceiling for fast peers is a judgement call, not a measurement: it
comfortably covers a phone photo and stays well inside what an LXMF link can
deliver over IP, but a relay-class peer with several hops may still be slow enough
to hit LXMF's own delivery timeouts. The visible progress states and the offline
queue are the mitigation; a resumable transfer protocol is the eventual answer and
is out of scope here.
