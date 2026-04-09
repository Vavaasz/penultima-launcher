# Penultima Launcher

Windows launcher for Penultima.

What it does:

- downloads and updates the public client feed from `Vavaasz/penultima-client`
- only updates managed client folders: `assets`, `bin`, and `sounds`
- keeps launcher state in AppData instead of writing manifests into the client root
- starts the client with production defaults for `ultimaotserv.online`

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
```

The first command rebuilds the public client feed from `D:\Server\Cliente-15.23-Prod`.

The second command rebuilds the launcher release and writes `D:\Server\_publish\Penultima-Launcher.zip`.
