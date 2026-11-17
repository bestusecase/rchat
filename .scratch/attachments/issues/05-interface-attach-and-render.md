# 05 — Interface: attach, pending chips, rendering and viewer

Status: done

## Scope

The shared interface, which is identical on desktop and Android apart from two
platform branches noted under 06.

- The plus button becomes live. On desktop it opens the system picker directly; on
  mobile it opens a two-item sheet, "Camera" and "Photo or file". Both use a plain
  multi-select file input, which the platform WebView already routes to the native
  picker.
- Chosen files appear as chips above the composer with a thumbnail or type glyph,
  the filename and a remove control. Upload happens at send time.
- The composer sends when there is text or at least one chip. Upload failure leaves
  the chips in place with their error and creates no message; the send button
  recovers without re-picking.
- Paste and drag-and-drop on desktop add to the same chips.
- Rendering: a single image at up to 320 px tall, aspect preserved, no padding, with
  the caption beneath it in the same bubble; two or more images as a two-column
  grid of 120 px crops with an overflow overlay beyond four; any non-image as a
  card with a type glyph, filename and human-readable size, and one save action.
- A full-screen viewer for images: centred image, close control, save action,
  Escape and click-outside to close.
- Messages with attachments are announced to assistive technology as image or file
  with their caption, not as empty bubbles.
- The conversation list preview reads the attachment kind, or the filename when
  there is exactly one file.

## Acceptance

- The plus opens the picker on desktop and the sheet on mobile; the sheet's camera
  entry opens the camera and its other entry opens the picker.
- Selecting three files shows three chips; removing one leaves two; sending uploads
  two and creates one message.
- An attachment with no caption sends; a message with neither text nor chips cannot
  be sent.
- An upload that fails leaves the chip visible with an error and no message row.
- A single image renders at most 320 px tall with its aspect ratio preserved.
- Four images render as a grid with no overlay; six render with an overlay naming
  the remainder.
- Tapping an image opens the viewer at full size; closing returns to the same
  scroll position.
- A non-image renders as a card with its name and size and no inline preview.
- The preview line for an image-only message reads as an image, and for one file
  reads that file's name, never a blank line.
- An evicted or absent attachment renders its reason in place of the image, in the
  same bubble, without disturbing the caption.
- A refused over-limit send surfaces the engine's reason verbatim.

## Notes

The viewer and the chips both consume the download route rather than holding bytes
in the interface, so the same code works for a 40 KB thumbnail and an 8 MB original.
The sheet exists only on mobile because a desktop has no camera; keep the branch to
that one decision.
