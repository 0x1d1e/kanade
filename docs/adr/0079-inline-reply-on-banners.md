# 79. A Banner takes an inline reply

Status: accepted. Builds on ADR 0007 (Banners) and ADR 0027 (the notification daemon). Refines the Banners' rule that nothing takes the keyboard.

## Context

Chat notifications let you answer in place in KDE, with the `inline-reply` action. Kanade's Banners are click-only; a reply needs a text field and the keyboard.

## Decision

Verified against KDE plasma-workspace `libnotificationmanager` (`server_p.cpp`, `notification.cpp`, master), the only implementation of the protocol:

- The server advertises the capability `inline-reply`. A sender puts the action key `inline-reply` in its action list; the server takes it out of the buttons as the reply action. Hints `x-kde-reply-placeholder-text` and `x-kde-reply-submit-button-text` are optional.
- A reply is the signal `NotificationReplied(uint id, string text)` on `org.freedesktop.Notifications` at `/org/freedesktop/Notifications`, sent to the sender's unique bus name, not broadcast. The notification then closes.

Kanade does the same:

- **Capability** `inline-reply` is advertised, and the action taken out of the buttons, only while the `banners` Module is on, since the field lives on a Banner. Otherwise the key stays an ordinary button, as before.
- **Field.** The Banner shows a pill-shaped `TextInput` under the actions, placeholder from the hint. Enter sends, ignoring blank text. The submit-button hint is not used.
- **Keyboard.** The Banners window asks for `Keyboard::OnDemand` only while a shown Banner has a field; a click on the field takes the keyboard. The click-only rule holds for actions.
- **Delivery.** `Notifications::reply` sends the signal to the notification's sender only, closes it unless resident, and the Banner goes. Text is cut to 2000 characters, kept nowhere and never logged.
- **No loss to the clock.** While the field has text its Banner has no timer (`Stack::draft`); an empty field starts the time afresh. A replacement version keeps the hold while it still takes a reply.

## Alternatives

**Keyboard `Exclusive` while a Banner shows.** Takes keys from the user's window on every message.

**A prompt in the Notifications Surface.** Doubles the UI; the Banner is where the message is read.

## Consequences

Moving focus to another output while typing drops the text, as the field is per pane. Not tested live here: the field, focus and delivery need a manual run (see the handoff).
