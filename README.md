# Penultima Launcher

Windows launcher for Penultima.

What it does:

- downloads and updates the public client feed from `Vavaasz/penultima-client`
- downloads and updates the website-hosted client feed from `ultimaotserv.online/downloads/client-feed`
- updates the launcher executable from the website-hosted `Penultima-Launcher.zip` on startup, with a manual button as a fallback
- only updates managed client folders: `assets`, `bin`, and `sounds`
- keeps launcher state in AppData instead of writing manifests into the client root
- backs up and restores official 15.23 UI state from `conf\clientoptions.json` and `characterdata\**\*.json`
- starts the client with production defaults for `ultimaotserv.online`
- launches only the selected folder's `bin\client.exe`; `client_launcher.exe` and nested client folders are not launch fallbacks
- keeps update, Force Update, full-map, and config repair work outside the Play button path
- minimizes the launcher itself to the system tray

For players:

- download the latest release zip
- extract it anywhere
- run `penultima-launcher.exe`
- let the launcher download or update the client automatically

Public client feed:

- [Penultima Client](https://github.com/Vavaasz/penultima-client)
- The launcher resolves the preferred runtime client feed from `https://ultimaotserv.online/downloads/penultima-downloads.json` under `client_feed`. The GitHub raw feed is a fallback only.
- If the direct `D:\Server\Cliente-15.23-Prod\bin\client.exe` works but the launcher-managed AppData client misses protobuf assets, compare `C:\Users\Waldir\AppData\Roaming\Penultima Launcher\game\assets\catalog-content.json` and `state\package.json` against `https://ultimaotserv.online/downloads/client-feed/package.json` before looking at server code.
- The visible `Play Client 15.23` path must only launch the selected folder's `bin\client.exe`. Client feed repair belongs to `Force Update`, startup/background checks, or explicit headless maintenance, not to the Play click itself.
- Client layout state is runtime data, not feed data. The launcher must preserve `conf\clientoptions.json` plus all `characterdata\**\*.json` files, including action bars, status bar, sidebars, analyzer widget state, and container widget layout. The managed feed must continue excluding `characterdata`; recovery happens through the launcher's `state\client-ui-state\latest` vault and automatic local discovery, not through manual player import.
- For the current 15.23 feed, the expected patched `client.exe` SHA-256 is `52449E00EBAE67F433333AC86708E85721C0A762CD2EBF2C6271D2AB8C9DBC98`. The client-editor PR #16 patch changes bytes at offset `0x30D254` from `75 0F E8 35 FF FF FF 48` to `EB 0F E8 35 FF FF FF 48`, but that binary patch does not suppress the visible BattlEye popup when the game server still sends CipSoft's client-check packet.
- Tibia 15.23 renders `ProtocolGame::sendClientCheck()` (`0x63`, `uint32 1`, `byte 1`) as the `clientcheck_disconnected` BattlEye dialog. If `login.php` already returns `anticheatprotection=false` and the popup still appears after character login, verify that the server login flow is not calling `player->sendClientCheck()` before changing launcher or asset code.
- The 15.23 client binary contains an `enableClientCheck` config key. Keep `[STARTUP] enableClientCheck=false` in shipped `conf\config.ini` as a client-side guard, but do not rely on it as the only fix; the server should not send the client-check packet for this deployment.
- Website boosted boss/creature images that use `lookTypeEx` must render through the local protobuf sprite endpoint, for example `getItemImageUrl(..., animate: true)` or `tools/sprite.php?type=item&id=<id>&animate=1`; the legacy `item_images_url/<id>.gif` path bypasses the 15.23 protobuf assets.
- The full minimap package must not be used as proof that client assets are current; `full_minimap.asset_file_count = 0` means it only updates `game\minimap`.
- Both first and additional 15.23 launches must execute exactly `<selected client folder>\bin\client.exe`. Do not fall back to `client_launcher.exe`, nested `*/bin/client.exe`, or generated secondary-client roots.
- A visible `Play Client 15.23` click must not refuse only because another `client.exe` is already running from that same folder. Existing clients are left alone; the click starts another direct `bin\client.exe` process.
- Before spawning `client.exe`, strip Windows extended-length prefixes such as `\\?\D:\...` back to normal `D:\...` paths. The command line and working directory should match a regular double-click from `bin\client.exe` as closely as possible.
- Do not save or apply client window placement after a `Play Client 15.23` launch. A stale maximized `state\client-window-state.json` can create a large `Default IME` child window over the map area, causing black flashes and mouse clicks to hit the overlay instead of the game.
- To build a clean local client ZIP from the real protobuf closure, use `python tools\clean_client_package.py --source "D:\Server\Tibia 15.23.bf9553-original-windows" --output "D:\Server\_publish\Tibia-15.23-local-clean.zip" --report "D:\Server\_publish\Tibia-15.23-local-clean.report.json"`. The tool keeps every `file` from `assets\catalog-content.json`, decodes `map-*.dat` protobuf map asset records for `resource_files.file_name`, decodes sound-bank protobuf strings from `sounds\catalog-sound.json`, fails before ZIP generation on missing references, and re-audits the generated ZIP for `extraFiles=0` and `missingFiles=0`.
- A selected client folder outside the launcher's managed AppData `game` folder is treated as a local/direct client. The launcher must skip website launcher updates, client feed checks, and Force Update for that folder, then launch its `bin\client.exe` as-is. This includes delayed startup/background checks after `Client Folder` changes, so the managed/local decision must be re-read before update work starts. This keeps `D:\Server\Tibia 15.23.bf9553-original-windows` usable for local login/debug without replacing it from the public feed.
- `Client Folder` must remain available while clients are open. Switching the selected folder only changes launcher state and does not mutate the old client folder; destructive/update actions such as `Force Update` still require no clients running for the selected folder.
- If the saved or selected folder is the client's `bin` directory, normalize it back to the client root before checking managed/local mode. A saved `...\game\bin` path makes the launcher search for `...\game\bin\bin\client.exe` and incorrectly disables Force Update as if the managed AppData client were an external local folder.
- If `penultima-downloads.json` provides `client_feed.bootstrap_sha256` but `client_feed.bootstrap_zip` has no query string, the launcher appends `?sha256=<hash>` before downloading the bootstrap ZIP. Without that cache-buster, a stale cached `Penultima-Client-Feed.zip` can pass through the old URL and fail with an expected/obtained size mismatch.
- Write `downloads/penultima-downloads.json` as UTF-8 without BOM. Windows PowerShell `Set-Content -Encoding utf8` can publish a BOM-prefixed JSON file that strict launcher metadata parsing may reject.
- `state\settings.json` must be read with a UTF-8 BOM tolerated. PowerShell can write a BOM, and without trimming it the launcher silently falls back to the managed AppData `game` folder instead of the selected local client.
- Full-map verification must avoid large stack buffers. The launcher hashes the minimap ZIP with a small streaming buffer because a 1 MiB stack buffer overflows the launcher main thread during `--full-map-once`.
- Local smoke-test flags are available for background maintenance: `--update-client-once`, `--full-map-once`, `--prepare-otclient-once`, and `--launch-client-once`.
- For a local multi-client smoke test without opening the launcher UI, write `state\settings.json` with the selected local `game_path`, run `penultima-launcher.exe --launch-client-count 3`, and inspect only the PIDs whose executable path is exactly inside that selected client's `bin` folder.
- To open a visible local launcher beside the production launcher, use an isolated `APPDATA` and pass `--instance-suffix <name>` so the single-instance lock does not signal the production launcher.
- The OTC button downloads `http://otcrp.com/downloads/penultima/launcher.zip`, extracts `OTCLauncher.exe` under `C:\Users\Waldir\AppData\Roaming\Penultima Launcher\otclient\penultima`, and that stub updates/runs the real OTC install from `C:\Users\Waldir\AppData\Local\otclient-premium\app`.

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
The launcher metadata points to the tracked `downloads/Penultima-Launcher.zip` with a SHA query string. `Penultima-Launcher-<version>.zip` may exist as an untracked archival copy, but do not point metadata at it unless that file is also explicitly synced to the VPS/public web root.
When only the client feed is republished, keep the existing `launcher` and `full_minimap` metadata in `penultima-downloads.json` unless those payloads are rebuilt too.

`install-client-feed-hook.ps1` installs a local `post-commit` hook in `D:\Server\Cliente-15.23-Prod` that runs `sounds\publish-website-client-assets.ps1`, so each client commit refreshes the website `client-feed`, bootstrap feed zip, portable client zip, and metadata from the current local client state.

`install-launcher-website-hook.ps1` installs a local `post-commit` hook in `D:\Server\Launcher` that runs `deploy-website-downloads.ps1 -SkipClient`, so each launcher commit refreshes `D:\Server\UniServerZ\www\downloads\Penultima-Launcher.zip`. If no signing certificate environment is configured, the hook passes `-AllowUnsignedLauncher` for local-only publishing.

VPS automation for website client assets now lives in `D:\Server\Cliente-15.23-Prod`, because `D:\Server\Launcher` is not deployed on the VPS.

Release policy:

- public launcher builds should be Authenticode-signed before publishing
- `publish-launcher-release.ps1` accepts either `PENULTIMA_SIGN_CERT_THUMBPRINT` or `PENULTIMA_SIGN_CERT_PATH`/`PENULTIMA_SIGN_CERT_PASSWORD`, and refuses unsigned public builds unless `-AllowUnsigned` is used for local-only testing
- `deploy-website-downloads.ps1` accepts the same signing inputs and refuses unsigned public launcher deploys unless `-AllowUnsignedLauncher` is passed for local-only testing
