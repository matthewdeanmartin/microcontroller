# Optional Wi-Fi setup plugin

Enable `wifi-setup` in the app's miniframework dependency. It is off by
default; disabled builds include no portal module, assets or setup worker.
It requires neither Angular nor external JavaScript/CSS downloads. The
phone uses its normal Wi-Fi settings to join the device's setup network,
then opens the local page. Captive-portal probes redirect to `/wifi-setup`;
if the phone does not open it automatically, visit `http://192.168.4.1/`.

The board initially tries its saved Wi-Fi for three minutes. With no saved
or built-in credentials, setup opens immediately. Connection failures after
startup get the same retry window before setup opens. The page lets users
change the window to 1–60 whole minutes; it is persisted in the existing
Wi-Fi NVS namespace as `retry_min` and used on subsequent boots/recovery.
Transient outages do not erase credentials. Startup attempts use at most
20 seconds for association/DHCP and 2–15 seconds of bounded backoff within
the overall window. Runtime recovery uses the existing nonblocking board
reconnection loop, checked every ten seconds.

## Integrating an app

```toml
miniframework = { path = "../../crates/miniframework", features = ["wifi-setup"] }
```

Use the existing board config constructors. These calls replace ordinary
`esp::start` and the bare app in `Site::new`:

```rust,ignore
use miniframework::{esp, wifi_setup::Options, Config, Site};

let mut config = esp::BoardConfig::s2("", "", "my-device");
config.saved_wifi = Some("app_wifi"); // NVS namespace: at most 15 characters
config.setup_network = Some("my-device-setup");
// Fill certificates, board resources, and other app settings as usual.
let options = Options {
    code: option_env!("MY_APP_SETUP_CODE"), // None disables the gate
    ..Default::default() // retry_minutes: 3
};
let board = esp::start_with_wifi_setup(config, peripherals.modem, options)?;
let app = board.wifi_portal(app)?;
let site = Site::new(Config::new("my-app", "my-device.local"), app, platform);
board.serve(site, |_| {})?;
```

`BoardConfig` has no new mandatory fields and existing `esp::start` users
keep their current setup behavior. The radio retry policy is enabled only
through `start_with_wifi_setup`; the page/API/task are enabled through
`board.wifi_portal(app)`. Call the latter once. The app stays accessible
normally after setup closes. Wrapper consumers can access their app through
`site.service.inner()`. A custom desktop/backend integration can create
`Arc<Controller<B>>`, wrap with `Portal::new(app, controller)`, and run
`controller.run_once()` from one app-owned worker.

If the app normally requires HTTPS, **only explicitly owned setup routes**
bypass that requirement while provisioning. Normal APIs still enforce the
app's HTTPS policy. Setup routes precede the app's static/SPA fallback, so
they work with an Angular bundle or an app that owns `/`. The optional
`Service::setup_route` hook also delegates through `RateLimited` wrappers.
App policies for normal CORS, schemas and metrics delegate as before.

## UI and radio responsibilities

Vanilla JavaScript drives scans, renders the list, offers manual/hidden SSID
entry and password visibility, validates inputs for feedback, polls progress,
handles failures, and displays the acquired address. SSIDs are inserted as
text, never HTML. Codes/passwords remain in page memory and request bodies/
headers; they are not stored in browser storage or put in URLs. The device
validates input again and owns all radio and NVS operations.

Only one scan/join/settings/close operation can be queued or running. HTTP
handlers return 202 and polls read cached state; they never acquire the
radio mutex. A second operation returns 409. Only a successful association
**and DHCP address** allow new credentials to replace saved credentials.
Wrong passwords keep the setup network available. The board keeps its AP
while joining the selected network; radio channel changes may briefly
disconnect the phone, so polling retries rather than restarting the join.

After connection, the UI displays the device's IPv4 address and asks the
phone user to switch back to home Wi-Fi. Setup closes automatically after
about a minute, or users can close it explicitly. Closing waits one second
after acknowledgement so the response can leave before the AP disappears.
Custom apps requiring HTTPS can use their own hostname/trust flow after
provisioning; the generic page displays an HTTP address for initial access.

## Optional compiled code

`Options { code: Some("your-code"), ..Default::default() }` enables the
lightweight gate. `code: None` turns it off. Values must be 1–64 printable
ASCII characters; an empty `Some("")` is a configuration error. The code
is compiled into firmware, never included in public HTML, metadata, or JS.
The page asks for it and sends `X-Wifi-Setup-Code` on protected requests.
All scan, status, join, retry-setting and close endpoints enforce it.

This is intentionally a lightweight local setup gate. The setup AP remains
open and uses HTTP; the code does not encrypt traffic. Mutation endpoints
also require a custom request header, reject foreign origins, and grant no
cross-origin access. The APIs/assets are unavailable after setup closes.

## Bounds and board resources

- One queued command; request bodies are at most 512 bytes and uncompressed
  JSON. Names are 1–32 UTF-8 bytes. Passwords are empty for open networks,
  8–63 bytes for passphrases, or exactly 64 hexadecimal characters.
- Scan storage on ESP is limited to 20 AP records. The UI list is bounded
  to 20 unique names, sorted by signal among those records. Errors are
  bounded to 160 characters. The board's radio determines supported bands
  and authentication; enterprise Wi-Fi provisioning is not implemented.
- The setup worker has a **12 KiB internal-RAM stack**, because it writes
  NVS. It must not use a PSRAM stack. Captive DNS uses the existing 6 KiB
  task. Controller state includes at most twenty 32-byte names, a single
  small command, and status strings, in addition to allocator metadata.
- HTML/JS are compiled as static assets and streamed from flash instead
  of constructing a full response buffer. They add no external dependency.
  Existing HTTP body/response/connection limits remain in force.

Firmware compilation does not validate radio behavior or heap headroom.
Before deployment, check `/api/v1/sys` memory numbers on the target and
exercise scans, wrong passwords, channel changes, DHCP failure, persistence,
router recovery and AP shutdown. Never reset/flash a board without approval.

## Housemetrics opt-in

The reference firmware has a forwarding `wifi-setup` feature. Provisioning
builds ignore baked-in Wi-Fi credentials and use `hm_wifi` NVS plus
`housemetrics-setup`; new boards open setup immediately. Existing builds
without the feature retain their Wi-Fi configuration. The optional compiled
code is `HOUSEMETRICS_WIFI_SETUP_CODE`, read from the environment or existing
`.env` configuration. Leave it unset to disable the gate.

After the usual web/certificate build prerequisites, build without flashing:

```sh
bash tools/build-firmware.sh apps/housemetrics housemetrics-esp32 --features https,wifi-setup
```

## Local preview and validation

From `crates/miniframework`:

```sh
cargo run --example wifi_setup --features wifi-setup
# Or enable the compiled demo code:
WIFI_SETUP_CODE=demo-code cargo run --example wifi_setup --features wifi-setup
```

Open `http://127.0.0.1:18091/`. This is a simulated radio, not a real Wi-Fi
scan. Its password is `engineering`, or choose the open guest network. It
persists only the retry setting in a temporary directory; override it with
`WIFI_SETUP_DEMO_DATA`. `WIFI_SETUP_DEMO_ADDR` selects a different loopback
listen address. `make check` includes feature/transport tests and JS syntax.

From `bench`, against a fresh running preview:

```sh
uv run --group dev python -m fmtbench.wifi_setup_smoke
# For the compiled-code preview:
uv run --group dev python -m fmtbench.wifi_setup_smoke --code demo-code
```

The browser test checks mobile rendering, scans, code enforcement, retry
settings, failed credentials, manual SSIDs, connection status and AP closure.
It accepts loopback URLs only because it changes settings and closes setup.
Screenshots are saved under `.local/wifi-setup/screens` for visual inspection.

The API is under `/wifi-setup/api`: public `GET meta`, protected `GET status`,
and `POST scan`, `POST join`, `POST retry`, `POST close`. POST requires
`X-Wifi-Setup-Request: 1`; join uses `{ssid,password,retry_minutes}` and retry
uses `{retry_minutes}`. Status reports `active`, `retry_minutes`, `phase`,
`ssid`, `address`, `error` and `networks`, with no credentials.
