# 08 — Device acceptance pass

Status: ready-for-agent

## Scope

The matrix that proves the platform behaviour no automated test covers, run once on
the Pixel 8 against a desktop daemon on the same LAN.

- Attach from the camera; attach from the gallery; attach three at once.
- Remove one chip before sending; send with a caption; send without one.
- Send to a desktop peer and receive the reverse; both directions show the image in
  the thread and open in the viewer.
- Save an image from the viewer and a PDF from its card; open both outside the app.
- Force the budget down and watch an old received attachment become "removed to
  save space" while its message text and time remain.
- Force the budget down further and watch an old sent attachment go the same way.
- Send an attachment larger than the peer's class permits and read the refusal.
- Have a peer send an attachment larger than the limit and confirm the message
  arrives with its placeholder.
- Background the app, receive an image, check the notification line and the lock
  screen.
- Rotate the device and background/foreground the app mid-conversation to confirm
  nothing in the thread is corrupted.
- Confirm the chat still scrolls smoothly with a photo-heavy conversation of at
  least thirty images.

## Acceptance

Every line above passes, and anything that does not is either fixed or recorded as
a known limitation with its reason.

## Notes

Smooth scrolling with many images is the check most likely to fail, and the reason
thumbnails exist; if it fails, the answer is a smaller thumbnail rather than
removing the feature.
