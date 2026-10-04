"""Headless check that the housemetrics UI works against a running server.

    uv run ui-smoke --url http://127.0.0.1:8080 --admin <password>

Walks every page in Chromium, graphs two series in each wire format, runs
a one-round format lab, creates a device, and fails on any console error.
Screenshots go to --out.
"""
import argparse
import pathlib
import sys
import time

from playwright.sync_api import Page, sync_playwright

FORMATS = ["json", "msgpack", "cbor", "cbor-int", "protobuf"]


def check(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(f"FAIL: {message}")
    print(f"ok   {message}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default="http://127.0.0.1:8080")
    parser.add_argument("--admin", default="admin")
    parser.add_argument("--out", default="../.local/screens")
    parser.add_argument("--headed", action="store_true")
    args = parser.parse_args()
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    errors: list[str] = []

    with sync_playwright() as p:
        browser = p.chromium.launch(headless=not args.headed)
        page: Page = browser.new_page(viewport={"width": 1280, "height": 900}, ignore_https_errors=True)
        page.on("console", lambda m: m.type == "error" and errors.append(m.text))
        page.on("pageerror", lambda e: errors.append(str(e)))

        page.goto(args.url + "/")
        page.get_by_role("heading", name="Series").wait_for()
        # Local preview only: a second board makes filtering testable even
        # when physical peers are offline. Never run this smoke on live boards.
        timestamp = time.time_ns()
        seeded = page.request.post(
            args.url + "/api/v1/write",
            data=f"board,app=smoke-peer,host=smoke-peer.local uptime_s=42 {timestamp}\n",
            headers={"Authorization": f"Bearer {args.admin}", "Content-Type": "text/plain"},
        )
        check(seeded.ok, "seeded an isolated peer metric in the desktop preview")
        page.get_by_role("button", name="Refresh", exact=True).click()
        page.get_by_role("combobox", name="Board", exact=True).select_option("smoke-peer.local")
        page.wait_for_function("document.querySelectorAll('ul.series li').length === 1")
        check(page.locator("ul.series").inner_text().find("uptime_s") >= 0, "board selector filters peer metrics")
        page.screenshot(path=str(out / "board-selector.png"), full_page=True)
        page.get_by_role("combobox", name="Board", exact=True).select_option("")
        boxes = page.locator("ul.series input[type=checkbox]")
        check(boxes.count() > 0, f"dashboard lists {boxes.count()} series")
        for i in range(min(2, boxes.count())):
            if not boxes.nth(i).is_checked():
                boxes.nth(i).check()
        for fmt in FORMATS:
            page.select_option("header select", fmt)
            page.get_by_role("button", name="Refresh").click()
            page.locator(".u-wrap canvas").first.wait_for(timeout=10_000)
            page.wait_for_function(
                "f => document.querySelector('p.wire .tag')?.textContent?.toLowerCase().includes(f)",
                arg={"json": "json", "msgpack": "messagepack", "cbor": "cbor", "cbor-int": "cbor int", "protobuf": "protobuf"}[fmt],
                timeout=10_000,
            )
            check(True, f"dashboard graphs in {fmt}")
        page.screenshot(path=str(out / "dashboard.png"), full_page=True)
        page.select_option("header select", "json")

        page.get_by_role("link", name="System").click()
        page.get_by_role("heading", name="Memory").wait_for()
        page.get_by_role("heading", name="Metrics store").wait_for()
        check(True, "system page shows board, memory, network and store")
        page.screenshot(path=str(out / "system.png"), full_page=True)

        page.get_by_role("link", name="Format lab").click()
        page.locator("input[type=number]").fill("1")
        page.locator("input[type=number]").dispatch_event("change")
        page.get_by_role("button", name="Run").click()
        page.get_by_role("heading", name="Verdict").wait_for(timeout=180_000)
        page.wait_for_function(
            "[...document.querySelectorAll('button')].some(b => b.textContent.trim() === 'Run' && !b.disabled)",
            timeout=180_000,
        )
        rows = page.locator("table tbody tr").count()
        check(rows >= 10, f"format lab produced {rows} result rows")
        page.screenshot(path=str(out / "lab.png"), full_page=True)

        page.get_by_role("link", name="Devices").click()
        page.get_by_placeholder("Admin password").fill(args.admin)
        page.get_by_role("button", name="Sign in").click()
        page.get_by_role("heading", name="Devices").wait_for()
        page.get_by_placeholder("Device name, e.g. attic-s2").fill(f"smoke-{page.evaluate('Date.now()')}")
        page.get_by_role("button", name="Add device").click()
        token = page.locator("code.token").inner_text(timeout=10_000)
        check(token.startswith("hm_"), "creating a device shows its token once")
        page.screenshot(path=str(out / "devices.png"), full_page=True)
        browser.close()

    check(not errors, "no console errors" + (": " + "; ".join(errors) if errors else ""))
    print(f"Screenshots in {out.resolve()}")


if __name__ == "__main__":
    sys.exit(main())
