# 06 — Shells: saving files, and the camera path a QR scanner will reuse

Status: done

## Scope

The two things the shared interface cannot do for itself.

Desktop shell:

- A command that copies a blob from the store to a path the user picks, so a save
  never routes bytes through the interface. Where the dialog comes from is a shell
  detail; the interface only names a hash and, on success, shows nothing but the
  result.
- Cancelling the dialog is not an error.

Mobile shell:

- A command that writes a blob into the system media store, so a saved file appears
  in the user's Downloads rather than in app-private storage, and cancelling or a
  platform refusal is reported without breaking the conversation.
- The camera capture is expressed once so that a later QR scanner reuses it rather
  than introducing a second intent path. No scanning in this change.

## Acceptance

By hand, on both platforms:

- Saving an image from the viewer puts a byte-identical copy at the chosen location;
  cancelling the dialog leaves nothing behind and shows no error.
- Saving a PDF delivers a byte-identical copy that opens in a system viewer.
- On the Pixel, a saved file appears in Downloads with its original name.
- Denying a permission the platform requires reports a legible failure and leaves
  the app usable.
- The camera entry produces a photo that can be attached and sent.

## Notes

Nothing here is unit-tested; the repo has no device harness and these are platform
declarations. The camera intent is shared deliberately because QR scanning was
named as the next consumer.
