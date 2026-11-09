# EchoTwin desktop UI (preview)

A clickable preview of the laptop app: dark theme, rounded window, sample data.

Open it in any browser:

```bash
xdg-open desktop/ui/index.html
```

Use the **Preview** switch above the window to see the *Connected*, *Offline* and
*Not paired* states. Everything is clickable: tabs, replies, sending messages and files
(drag files onto the window), settings, unpairing and pairing.

## Files

| File | Purpose |
|---|---|
| `index.html` | window structure |
| `styles.css` | design tokens (colours, radii) and components |
| `app.js` | state, rendering and interactions; talks only to `window.PhoneLinkAPI` |
| `mock-api.js` | sample data with the same API the real backend will expose |
| `qr-sample.js` | a real QR code for the pairing screen; it points at 192.0.2.1, a reserved address, so it can't pair with anything |

The desktop app will load these same files in a Tauri window (frameless and transparent,
so the corners stay rounded) and swap `mock-api.js` for a backend that talks to
`phonelinkd`. Window buttons and "Show in folder" only show a hint in the preview.

## In the desktop app

`desktop/app` (Tauri) compiles these files into the `phonelink-app` binary and serves them
through Tauri's own protocol: there is no web server and nothing listens on localhost.
`tauri-api.js` then replaces the sample data with the real service. Features that the phone
doesn't provide yet are hidden or labelled "coming in the next update".
`phonelink-app --demo` shows the sample data inside the real window.
