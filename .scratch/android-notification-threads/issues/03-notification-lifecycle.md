# 03 — Notification lifecycle

Status: ready-for-agent

## Scope

Keep the shade consistent with what the running process believes.

- Cancel the summary and every child when an activity resumes, replacing the
  current single-notification cancel.
- Cancel all of the app's notifications while the service starts, before it
  promotes itself to the foreground, so a sticky restart never leaves threads the
  new process cannot account for.
- Cancel children for conversations absent from the latest snapshot, so reading a
  conversation or trimming it removes its notification.
- Track posted child ids on the Kotlin side only for the purpose of cancelling
  what is no longer in the snapshot.

## Acceptance

- Opening the app from a notification leaves an empty shade except for the
  ongoing service entry.
- Force-stopping the app, receiving a message, letting the system restart the
  service, then backgrounding again shows either no message notification or one
  consistent with the new snapshot — never a mix.
- Reading one of two waiting conversations removes only that conversation's
  child and updates the summary, or removes everything if the app was opened.
- The foreground-service notification is never cancelled by any of this.
