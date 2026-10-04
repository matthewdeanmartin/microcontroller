"""Generate an ignored C header using existing local credentials, without printing them."""
import ast
import json
import os
from pathlib import Path

root = Path(__file__).resolve().parent
ssid, password = os.getenv("SCREEN_INFO_WIFI_SSID"), os.getenv("SCREEN_INFO_WIFI_PASSWORD")
if ssid is None or password is None:
    for source in (root / "config.py", root.parent / "hello_wifi_s3_py/config.py", root.parent / "hello_wifi_py/config.py"):
        if not source.exists():
            continue
        values = {}
        for node in ast.parse(source.read_text()).body:
            if isinstance(node, ast.Assign):
                for target in node.targets:
                    if isinstance(target, ast.Name) and target.id in ("WIFI_SSID", "WIFI_PASSWORD"):
                        values[target.id] = ast.literal_eval(node.value)
        if "WIFI_SSID" in values and "WIFI_PASSWORD" in values:
            ssid = ssid if ssid is not None else values["WIFI_SSID"]
            password = password if password is not None else values["WIFI_PASSWORD"]
            break
if not isinstance(ssid, str) or not ssid or not isinstance(password, str):
    raise SystemExit("Set SCREEN_INFO_WIFI_SSID and SCREEN_INFO_WIFI_PASSWORD, or create ignored config.py.")
if len(ssid.encode()) > 32 or len(password.encode()) > 63:
    raise SystemExit("Wi-Fi SSID/password exceeds ESP-IDF limits.")
# Octal escapes keep UTF-8 bytes exact and cannot introduce C preprocessor syntax.
def literal(value):
    return '"' + ''.join('\\%03o' % byte for byte in value.encode()) + '"'
(root / "wifi_config.h").write_text(
    "#pragma once\n#define WIFI_SSID " + literal(ssid) + "\n#define WIFI_PASSWORD " + literal(password) + "\n"
)
print("Wi-Fi configuration ready (values hidden; wifi_config.h is ignored).")
