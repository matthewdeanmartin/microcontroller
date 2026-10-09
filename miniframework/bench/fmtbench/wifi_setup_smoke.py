"""Exercise the simulated Wi-Fi portal in Chromium; loopback previews only.

Run the wifi_setup Rust example first, then:
    uv run --group dev python -m fmtbench.wifi_setup_smoke
"""
import argparse
import pathlib
from urllib.parse import urlparse

from playwright.sync_api import expect, sync_playwright


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default="http://127.0.0.1:18091")
    parser.add_argument("--code")
    parser.add_argument("--out", default="../.local/wifi-setup/screens")
    args = parser.parse_args()
    if urlparse(args.url).hostname not in {"127.0.0.1", "localhost", "::1"}:
        parser.error("This test submits credentials and closes setup; use the loopback demo only")
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    errors: list[str] = []
    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page(viewport={"width": 390, "height": 844})
        page.on("pageerror", lambda error: errors.append(str(error)))
        page.goto(args.url + "/")
        expect(page).to_have_url(args.url + "/wifi-setup")
        expect(page.get_by_role("heading", name="Connect to your Wi-Fi")).to_be_visible()
        if args.code:
            expect(page.locator("#gate")).to_be_visible()
            page.locator("#code").fill("incorrect-code")
            page.get_by_role("button", name="Continue", exact=True).click()
            expect(page.locator("#message")).to_contain_text("Enter the setup code")
            page.locator("#code").fill(args.code)
            page.get_by_role("button", name="Continue", exact=True).click()
        else:
            expect(page.locator("#gate")).to_be_hidden()
        expect(page.locator("#networks option")).to_have_count(4)
        expect(page.locator("#connect")).to_be_enabled()
        page.screenshot(path=str(out / "networks-mobile.png"), full_page=True)
        page.get_by_text("Connection settings", exact=True).click()
        page.locator("#retry-minutes").fill("5")
        page.get_by_role("button", name="Save retry setting", exact=True).click()
        headers = {"X-Wifi-Setup-Code": args.code} if args.code else {}
        expect(page.locator("#connect")).to_be_enabled()
        response = page.request.get(args.url + "/wifi-setup/api/status", headers=headers)
        assert response.json()["retry_minutes"] == 5
        page.locator("#networks").select_option("Engineering lab")
        page.locator("#password").fill("bad-password")
        page.get_by_role("button", name="Connect device", exact=True).click()
        expect(page.locator("#message")).to_contain_text("Could not connect")
        expect(page.locator("#settings")).to_be_visible()
        expect(page.locator("#password")).to_have_value("")
        assert page.request.get(args.url + "/wifi-setup/api/status", headers=headers).json()["ssid"] == ""
        page.screenshot(path=str(out / "failed-join-mobile.png"), full_page=True)
        page.locator("#manual").check()
        name = "<img src=x onerror=alert(1)>"
        page.locator("#ssid").fill(name)
        page.locator("#password").fill("engineering")
        page.get_by_role("button", name="Connect device", exact=True).click()
        expect(page.locator("#success")).to_be_visible()
        expect(page.locator("#address")).to_have_attribute("href", "http://127.0.0.1/")
        assert page.request.get(args.url + "/wifi-setup/api/status", headers=headers).json()["ssid"] == name
        assert page.evaluate("document.documentElement.scrollWidth <= innerWidth")
        page.screenshot(path=str(out / "connected-mobile.png"), full_page=True)
        page.get_by_role("button", name="Close setup Wi-Fi", exact=True).click()
        expect(page.locator("#message")).to_contain_text("Setup closed")
        # The worker waits one second before dropping the AP.
        page.wait_for_timeout(1200)
        assert page.request.get(args.url + "/wifi-setup/api/status", headers=headers).status == 404
        page.goto(args.url + "/")
        expect(page.locator("body")).to_contain_text("App is running")
        assert not errors, errors
        browser.close()
    print("Wi-Fi UI passed: mobile layout, code gate, scans, saved retry policy, failed join, hidden SSID, successful join, and close.")


if __name__ == "__main__":
    main()
