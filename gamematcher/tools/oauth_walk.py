"""Browser walk for Sprint 6 ("Sign in with Mastodon") on a FRESH desktop
Game Matcher, against tools/mock_mastodon.py (no real instance involved).

    uv run tools/mock_mastodon.py --port 8099 &
    cd ../miniframework/bench
    uv run --group dev python ../../gamematcher/tools/oauth_walk.py http://127.0.0.1:8088 http://127.0.0.1:8099 <screenshots>
"""
import re
import sys
from playwright.sync_api import sync_playwright

base, instance, shots = sys.argv[1], sys.argv[2], sys.argv[3]
errors = []

with sync_playwright() as p:
    browser = p.chromium.launch()
    dad = browser.new_context(viewport={"width": 390, "height": 860}).new_page()
    dad.on("pageerror", lambda e: errors.append(f"dad: {e}"))
    dad.goto(base + "/")
    dad.wait_for_url("**/setup")
    dad.get_by_label("Email").fill("dad@home")
    dad.get_by_label("Your name").fill("Dad")
    dad.get_by_label("PIN (4–8 digits)").fill("1234")
    dad.get_by_label("PIN again").fill("1234")
    dad.get_by_role("button", name="Create admin account").click()
    dad.get_by_text("Hi, Dad").wait_for()

    # Admin adds the (mock) household instance.
    dad.get_by_role("link", name="Admin").click()
    dad.get_by_label("Instance").fill(instance)
    dad.get_by_label("Name").fill("Home Mastodon")
    dad.get_by_label("Who").select_option("household")
    dad.get_by_role("button", name="Add", exact=True).click()
    dad.get_by_text(instance).wait_for()
    dad.screenshot(path=f"{shots}/s6-admin.png", full_page=True)

    # Sam has never been here: Sign in with Mastodon makes the account.
    sam = browser.new_context(viewport={"width": 390, "height": 860}).new_page()
    sam.on("pageerror", lambda e: errors.append(f"sam: {e}"))
    sam.goto(base + "/signin")
    sam.screenshot(path=f"{shots}/s6-signin.png", full_page=True)
    sam.get_by_role("button", name="Sign in with Home Mastodon").click()
    sam.get_by_text("Hi, Sam (mock)").wait_for(timeout=20_000)
    print("sam signed in via mock, at", sam.url)

    # Dad links his own account from the profile page.
    dad.get_by_role("link", name=re.compile(r"^Dad$")).click()
    dad.get_by_role("button", name="Link Home Mastodon").click()
    dad.wait_for_url("**/me", timeout=20_000)
    try:
        dad.get_by_text("Linked to").wait_for(timeout=5_000)
        print("dad:", dad.get_by_text("Linked to").inner_text())
    except Exception:
        # The mock has one account (@sam), already Sam's: linking must refuse.
        print("dad link refused as expected:", dad.locator(".error").first.inner_text())
    dad.screenshot(path=f"{shots}/s6-profile.png", full_page=True)
    browser.close()
print("page errors:", errors)
