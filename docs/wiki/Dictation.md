# Dictation

The `dictation` Module turns speech into text with the Gemini API (ADR 0111). It is off by default, and it needs a Gemini API key and your consent before it records or sends anything.

> Status: recording, the request and inserting the text are built. The Gemini request has **not been checked against a live key**, and typing with `wtype` has not been run live: the first real dictation may fail with a refusal or an unreadable answer until the request is adjusted (ADR 0111, Unverified).

## Privacy: read this first

Each dictation sends its **audio to Google's servers**. Google's terms for the **free tier** allow it to use what you send to improve its products, and human reviewers may read it (check Google's current terms, which Kanade cannot vouch for). Do not dictate passwords, private or confidential matter, or things about other people while using a free-tier key. Paid usage is under different terms, which you should read; Kanade cannot tell which kind of key you gave it.

Kanade records only after you start a dictation, keeps the audio in memory (never on disk), sends it in one request when you stop, and forgets it. The microphone indicator on the island lights while it records, like for any program using the microphone, but only with the `privacy` Module on and `privacy.indicators = true`; without them nothing on screen says the microphone is open besides the Dictation Activity.

## Setup

1. Make a key in [Google AI Studio](https://aistudio.google.com/apikey).
2. Give it to Kanade: `kanade dictate key`. It reads the key from stdin (typed with no echo at a terminal, or piped, like `secret-tool lookup ... | kanade dictate key`), never from an argument, and keeps it in the Secret Service (GNOME Keyring, KWallet, KeePassXC), never in the config, a log or `kanade ui`.
3. Accept the disclosure above: `kanade dictate consent` prints it and asks you to type `I agree`. It must be typed at a terminal, so no script or keybind can accept for you. Kanade records the acceptance in `$XDG_STATE_HOME/kanade/dictation-consent`, readable by you only; if the disclosure changes, you are asked again.
4. Turn the Module on in the config: `[modules]` `dictation = true` (it takes a restart).

`kanade dictate forget` deletes the key and withdraws the consent. Consent is checked again just before the audio is sent, so a dictation recorded before you ran it is dropped, not sent; one already sent cannot be recalled.

Nothing records or sends without both the key and the acceptance. `kanade dictate status` says how a dictation stands and whether the gate is `ready`, has `no key` or has `no consent` (it never shows the key).

## Using it

Bind the toggle in niri:

```kdl
Mod+Alt+Y hotkey-overlay-title="Island: Dictate" { spawn "kanade" "dictate" "toggle"; }
```

The first press starts recording, the second stops it and sends it. A recording stops by itself at 120 seconds and is sent as it is. The island shows the recording with the seconds heard (Stop and Cancel on Peek), then that it transcribes (Discard), then what was done with the text or why it failed. Before you stop, `kanade dictate cancel` (or Cancel) drops it and the audio with it. Once you stopped it, the audio is already on its way to Google and cannot be recalled: Discard (and `cancel`) only throws the answer away, and nothing is inserted. A start is refused while the session is locked, and a recording not yet stopped when the session locks is cancelled ("Cancelled: the session locked"), so the microphone is not left open behind the lock screen, and the toggle is refused for the moment the key and consent are being read.

By default the text goes to the clipboard (`wl-copy`, from wl-clipboard), so paste it where you want it. It reaches `wl-copy` on its stdin, not in the arguments other programs can read. Keep in mind that a clipboard history, yours or Kanade's `clipboard` Module, then holds it.

## Putting the text in the window

`[dictate]` `insert` says where the text goes:

- `"copy"` (default): the clipboard.
- `"paste"`: the clipboard (as the same one line `type` types), then Ctrl+V sent to the focused window with `wtype`. The text stays on the clipboard. Terminals paste with Ctrl+Shift+V, so use `"type"` there.
- `"type"`: typed into the focused window with `wtype`, as one line (line breaks, tabs and any other control character such as Escape or Ctrl-C become a space, so a terminal is never told to run what you said).

Kanade types or pastes only into the window you started the dictation in, as far as it can tell: it checks again right before sending the keys, but focus can still move in the instant between that check and the compositor acting on them, so for text you cannot let leak, use `copy` or list the app in `never_in`. It falls back to the clipboard, marked sensitive (`wl-copy --sensitive`, so clipboard managers that honor it skip it), and the island says why (`Copied; the window changed`), when:

- the session is locked (the text is copied; nothing is typed at the lock screen);
- another window has the focus when the text is ready, or the focused window is unknown;
- the focused window's app id is in `never_in` (ids as in `niri msg windows`, compared without case; empty by default, add your password manager, e.g. `never_in = ["org.keepassxc.KeePassXC"]`);
- `wtype` is not installed, or fails.

The island always says what was done: `Typed`, `Pasted` or `Copied`, with the reason for a fallback. The text is never written to a log. Both settings are in the Settings app under Notifications & privacy.

## Limits

- Gemini API only; no local model, no streaming. The text appears after you stop.
- 120 seconds per dictation.
- Needs a network, `pw-record` (PipeWire) and the Secret Service; `wl-copy` for the clipboard, `wtype` for `type` and `paste`.
- A recording under about 0.2 seconds is not sent. A failed or cancelled dictation keeps nothing.
- One dictation at a time; `toggle` while it transcribes is refused until it ends or you discard it.
- The free tier's quotas apply; a rate limit shows as such.
- When Google answers with an error, the island says so with the HTTP status for a server failure ("Gemini is unavailable (503); try again") or a refusal ("Gemini refused the request (400)"), and the log (stderr) gets one line with Google's own words: `kanade: dictation: Gemini answered 503: UNAVAILABLE <message>`, the message with your key cut out, on one line and capped. A network failure logs only its kind (timeout, host not found, TLS error), never an address or a header. Nothing is retried.
- An answer with no text in it shows as "No speech heard". When Google's answer cannot be read as a transcript at all, or has none, Kanade writes one line to its log (stderr) with the shape of the answer: the HTTP status, its top-level field names and its step types, never the text, a header, your audio or the key.
- A transcript is the model's reading, inserted as it came.
