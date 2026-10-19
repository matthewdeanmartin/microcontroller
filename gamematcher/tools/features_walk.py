"""Regression walk against an isolated desktop server. Never point at a live board."""
import datetime
import pathlib
import sys
import time
from playwright.sync_api import sync_playwright, expect

base = sys.argv[1]
shots = pathlib.Path(sys.argv[2])
shots.mkdir(parents=True, exist_ok=True)
errors = []

with sync_playwright() as pw:
    browser = pw.chromium.launch()
    host = browser.new_context(viewport={"width": 1100, "height": 900})
    guest = browser.new_context()
    def api(context, method, path, body=None):
        result = context.request.fetch(base + path + ("&" if "?" in path else "?") + "fmt=json",
            method=method, data=body, headers={"Origin": base})
        assert result.ok, (path, result.status, result.text())
        return result.json()
    api(host, "POST", "/api/v1/setup", {"email": "host@test.lan", "display_name": "Host", "pin": "1234"})
    kid = api(guest, "POST", "/api/v1/auth/signup", {"email": "guest@test.lan", "display_name": "Guest", "pin": "1234"})
    game = api(host, "POST", "/api/v1/games", {"title": "Catan"})
    page = host.new_page()
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.goto(base + "/plans/new")
    page.get_by_role("checkbox", name="Catan", exact=True).check()
    page.get_by_role("button", name="Add a time", exact=True).click()
    tomorrow = (datetime.date.today() + datetime.timedelta(days=1)).isoformat()
    page.get_by_label("Day", exact=True).fill(tomorrow)
    page.get_by_role("button", name="Add a time", exact=True).click()
    expect(page.get_by_label("From", exact=True).nth(1)).to_have_value("20:00")
    page.get_by_label("Title", exact=False).fill("Weekly Catan")
    page.get_by_label("Frequency").select_option("weekly")
    page.get_by_label("Total occurrences").fill("3")
    page.get_by_role("button", name="Post plan", exact=True).click()
    page.wait_for_url("**/plans/1")
    expect(page.get_by_role("heading", name="Best times", exact=True)).to_be_visible()
    p = api(host, "GET", "/api/v1/plans/1")
    assert len(p["series"]) == 3
    api(guest, "PUT", "/api/v1/plans/1/response", {"overall": "in", "times": [
        {"time": p["times"][0]["id"], "answer": "yes"}, {"time": p["times"][1]["id"], "answer": "no"}]})
    page.reload()
    expect(page.get_by_text("Cannot make it: Guest", exact=True)).to_be_visible()
    page.get_by_role("button", name="Choose this time", exact=True).first.click()
    expect(page.get_by_role("button", name="Chosen below", exact=True)).to_be_visible()
    page.screenshot(path=str(shots / "best-times.png"), full_page=True)
    page.get_by_role("link", name="Occurrence 2", exact=True).click()
    page.wait_for_url("**/plans/2")
    expect(page.locator("gm-thread")).to_be_visible()
    expect(page.get_by_role("link", name="Occurrence 1", exact=True)).to_be_visible()

    p = api(host, "POST", "/api/v1/plans", {"title": "Finished game", "games": [game["id"]],
        "times": [{"start": int(time.time()) - 120, "end": 0}]})
    api(guest, "PUT", f'/api/v1/plans/{p["id"]}/response', {"overall": "in"})
    api(host, "POST", f'/api/v1/plans/{p["id"]}/lock', {"time": p["times"][0]["id"], "games": [game["id"]]})
    page.goto(base + f'/plans/{p["id"]}')
    page.get_by_label("How did it go?").fill("Great game. A tie!")
    page.get_by_role("checkbox", name="Host won", exact=True).check()
    page.get_by_role("checkbox", name="Guest won", exact=True).check()
    page.get_by_role("button", name="Save report", exact=True).click()
    expect(page.get_by_role("heading", name="Post-game report", exact=True)).to_be_visible()
    expect(page.get_by_text("Winners: Host, Guest", exact=False)).to_be_visible()
    page.screenshot(path=str(shots / "report.png"), full_page=True)

    page.get_by_role("link", name="Status", exact=True).click()
    expect(page.get_by_role("heading", name="Status", exact=True)).to_be_visible()
    expect(page.get_by_role("heading", name="Public Mastodon bot", exact=True)).to_be_visible()
    page.screenshot(path=str(shots / "status.png"), full_page=True)
    page.get_by_role("link", name="Configure email and bots").click()
    expect(page.get_by_role("heading", name="Household Mastodon bot", exact=True)).to_be_visible()
    expect(page.locator("gm-bot").nth(1).get_by_label("Instance", exact=True)).to_have_value("https://mastodon.social")
    page.set_viewport_size({"width": 390, "height": 860})
    page.screenshot(path=str(shots / "bots-mobile.png"), full_page=True)
    assert not errors, errors
    print("PASS: recurring plan, hourly defaults, rankings, series navigation, report winners, Status, two bot forms")
    browser.close()
