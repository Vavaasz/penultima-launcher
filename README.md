# Penultima Launcher

Windows launcher for Penultima.

What it does:

- downloads and updates the public client feed from `Vavaasz/penultima-client`
- downloads and updates the website-hosted client feed from `ultimaotserv.online/downloads/client-feed`
- only updates managed client folders: `assets`, `bin`, and `sounds`
- keeps launcher state in AppData instead of writing manifests into the client root
- starts the client with production defaults for `ultimaotserv.online`
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
powershell -ExecutionPolicy Bypass -File .\publish-client-feed.ps1
powershell -ExecutionPolicy Bypass -File .\publish-launcher-release.ps1
powershell -ExecutionPolicy Bypass -File .\deploy-website-downloads.ps1
```

The first command rebuilds the public client feed from `D:\Server\Cliente-15.23-Prod`.

The second command rebuilds the launcher release and writes `D:\Server\_publish\Penultima-Launcher.zip`.

The third command republishes the launcher zip, client feed, and portable client zip directly into `D:\Server\UniServerZ\www\downloads` from your local workstation.

VPS automation for website client assets now lives in `D:\Server\Cliente-15.23-Prod`, because `D:\Server\Launcher` is not deployed on the VPS.

Release policy:

- public launcher builds should be Authenticode-signed before publishing
- `publish-launcher-release.ps1` accepts either `PENULTIMA_SIGN_CERT_THUMBPRINT` or `PENULTIMA_SIGN_CERT_PATH`/`PENULTIMA_SIGN_CERT_PASSWORD`, and refuses unsigned public builds unless `-AllowUnsigned` is used for local-only testing
- `deploy-website-downloads.ps1` accepts the same signing inputs and refuses unsigned public launcher deploys unless `-AllowUnsignedLauncher` is passed for local-only testing
