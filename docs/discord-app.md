# Discord application for Mixtape

Mixtape's "Listening to" status needs only the **Application ID**, which goes into
`.secrets/discord.json` as `{"appId": "..."}`. Everything below is optional polish.

## General Information

| Field | What to put |
|---|---|
| **App Icon** | `assets/discord-mixtape.png` (1024×1024) |
| **Name** | `Mixtape` — Discord shows it in the status: "Listening to Mixtape" |
| **Description** | see below |
| **Tags** | `music`, `youtube music`, `music player`, `rich presence`, `windows` |
| Interactions Endpoint URL | leave empty (bots only) |
| Linked Roles Verification URL | leave empty (servers only) |
| Terms of Service URL | leave empty |
| Privacy Policy URL | leave empty |

**Description** (fits the 400-character limit):

```
Mixtape is a fast YouTube Music player for Windows with an offline library, endless mixes, synced lyrics and listening stats. This application only shows what you're listening to in your Discord profile: title, artist, cover and time. It has no bot, reads no messages and asks for no permissions.
```

## Rich Presence → Art Assets

| Field | What to put |
|---|---|
| **Image** | `assets/discord-mixtape.png` |
| **Asset name (key)** | `mixtape` — the app uses it as the picture for local tracks without a YouTube cover |

Rich Presence → Visualizer can stay untouched. Bot, OAuth2, Installation and Webhooks
pages are not used.
