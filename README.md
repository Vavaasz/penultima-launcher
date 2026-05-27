# Penultima Launcher

Windows launcher for Penultima.

What it does:

- downloads and updates the public client feed from `Vavaasz/penultima-client`
- downloads and updates the website-hosted client feed from `ultimaotserv.online/downloads/client-feed`
- updates the launcher executable from the website-hosted `Penultima-Launcher.zip` on startup, with a manual button as a fallback
- only updates managed client folders: `assets`, `bin`, and `sounds`
- keeps launcher state in AppData instead of writing manifests into the client root
- starts the client with production defaults for `ultimaotserv.online`
- resolves `client.exe` before `client_launcher.exe` for both direct and nested client folders
- checks launcher/client updates at startup before play, and normalizes risky mouse cursor options before launching the client
- minimizes the launcher itself to the system tray

For players:

- download the latest release zip
- extract it anywhere
- run `penultima-launcher.exe`
- let the launcher download or update the client automatically

Public client feed:

- [Penultima Client](https://github.com/Vavaasz/penultima-client)

Local publish helpers:

```powershell
powershell -ExecutionPolicy Bypass -File .\publish-client-artifacts.ps1
powershell -ExecutionPolicy Bypass -File .\publish-client-feed.ps1
powershell -ExecutionPolicy Bypass -File .\publish-launcher-release.ps1
powershell -ExecutionPolicy Bypass -File .\deploy-website-downloads.ps1
powershell -ExecutionPolicy Bypass -File .\install-client-feed-hook.ps1
powershell -ExecutionPolicy Bypass -File .\install-launcher-website-hook.ps1
```

The first command publishes the public client feed and the website `downloads` payload from the same `D:\Server\Cliente-15.23-Prod` commit, so the launcher feed and portable zip stay aligned.

The second command rebuilds only the public client feed from `D:\Server\Cliente-15.23-Prod`.

The third command rebuilds the launcher release and writes `D:\Server\_publish\Penultima-Launcher.zip`.

The fourth command republishes the launcher zip, client feed, and portable client zip directly into `D:\Server\UniServerZ\www\downloads` from your local workstation.
It also writes `penultima-downloads.json`, which the launcher's `Update Launcher` button uses to find, verify, stage, replace, and restart the launcher executable.
When only the client feed is republished, keep the existing `launcher` and `full_minimap` metadata in `penultima-downloads.json` unless those payloads are rebuilt too.

`install-client-feed-hook.ps1` installs a local `post-commit` hook in `D:\Server\Cliente-15.23-Prod` that runs `sounds\publish-website-client-assets.ps1`, so each client commit refreshes the website `client-feed`, bootstrap feed zip, portable client zip, and metadata from the current local client state.

`install-launcher-website-hook.ps1` installs a local `post-commit` hook in `D:\Server\Launcher` that runs `deploy-website-downloads.ps1 -SkipClient`, so each launcher commit refreshes `D:\Server\UniServerZ\www\downloads\Penultima-Launcher.zip`. If no signing certificate environment is configured, the hook passes `-AllowUnsignedLauncher` for local-only publishing.

VPS automation for website client assets now lives in `D:\Server\Cliente-15.23-Prod`, because `D:\Server\Launcher` is not deployed on the VPS.

Release policy:

- public launcher builds should be Authenticode-signed before publishing
- `publish-launcher-release.ps1` accepts either `PENULTIMA_SIGN_CERT_THUMBPRINT` or `PENULTIMA_SIGN_CERT_PATH`/`PENULTIMA_SIGN_CERT_PASSWORD`, and refuses unsigned public builds unless `-AllowUnsigned` is used for local-only testing
- `deploy-website-downloads.ps1` accepts the same signing inputs and refuses unsigned public launcher deploys unless `-AllowUnsignedLauncher` is passed for local-only testing
