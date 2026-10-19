"""Browser walk for Sprints 1-5 on a FRESH Game Matcher (desktop or board).

Two people in two browsers (390 px phones): Dad claims admin, adds games
(one duplicate), stars one, posts a plan with two times; Sam signs up as a
guest and answers; Dad locks it in; Sam sees the notification and
comments; Dad subscribes to his calendar feed, which is fetched and parsed
with the `icalendar` package. Saves screenshots. It leaves accounts
behind: on a board, `make flash` afterwards.

Uses Playwright from miniframework's bench environment:
    cd ../miniframework/bench
    uv run --group dev --with icalendar python ../../gamematcher/tools/ui_walk.py http://192.168.1.163 <screenshot dir>
"""
import re
import sys
from playwright.sync_api import sync_playwright, expect

base, shots = sys.argv[1], sys.argv[2]
errors = []


def watch(page, who):
    page.on("console", lambda m: errors.append(f"{who}: {m.text}") if m.type == "error" and "401" not in m.text else None)
    page.on("pageerror", lambda e: errors.append(f"{who} pageerror: {e}"))


with sync_playwright() as p:
    browser = p.chromium.launch()
    dad = browser.new_context(viewport={"width": 390, "height": 860}).new_page()
    watch(dad, "dad")
    dad.goto(base + "/")
    dad.wait_for_url("**/setup")
    dad.get_by_label("Email").fill("dad@home")
    dad.get_by_label("Your name").fill("Dad")
    dad.get_by_label("PIN (4–8 digits)").fill("1234")
    dad.get_by_label("PIN again").fill("1234")
    dad.get_by_role("button", name="Create admin account").click()
    dad.get_by_text("Hi, Dad").wait_for()

    # Sprint 2: catalog.
    dad.get_by_role("link", name="Games", exact=True).click()
    for title, lo, hi in [("Catan", "3", "4"), ("Wingspan", "1", "5"), ("Twilight Imperium", "3", "6")]:
        dad.get_by_role("button", name="Add a game").click()
        dad.get_by_label("Title").fill(title)
        dad.get_by_label("Fewest players").fill(lo)
        dad.get_by_label("Most players").fill(hi)
        dad.get_by_role("button", name="Add", exact=True).click()
        dad.get_by_role("link", name=title).wait_for()
    dad.get_by_role("button", name="Add a game").click()
    dad.get_by_label("Title").fill("catan")
    dad.get_by_role("button", name="Add", exact=True).click()
    print("duplicate:", dad.get_by_role("alert").inner_text())
    dad.get_by_role("button", name="Close").click()
    dad.get_by_role("button", name="Into Catan").click()
    expect(dad.get_by_role("button", name="Not into Catan")).to_be_visible()
    dad.screenshot(path=f"{shots}/s2-games.png", full_page=True)

    # Sprint 3: a plan with two games and two times.
    dad.get_by_role("link", name="Home", exact=True).click()
    dad.get_by_role("link", name="New plan").click()
    dad.get_by_label("Catan").check()
    dad.get_by_label("Wingspan").check()
    dad.get_by_role("button", name="Sat 2–6 pm").click()
    dad.get_by_role("button", name="Sun 1 pm").click()
    dad.get_by_label("Title").fill("Saturday game night")
    dad.get_by_role("button", name="Post plan").click()
    dad.wait_for_url(re.compile(r"/plans/\d+$"))
    plan_url = dad.url
    dad.get_by_role("radio", name="In").click()
    dad.get_by_role("radio", name="Yes").first.click()
    dad.get_by_role("checkbox", name="Catan", exact=True).check()
    # Answers save themselves a moment after the last tap.
    dad.get_by_text("Saved ✓").wait_for()

    # A guest signs up in another browser and answers.
    sam = browser.new_context(viewport={"width": 390, "height": 860}).new_page()
    watch(sam, "sam")
    sam.goto(base + "/signup")
    sam.get_by_label("Email").fill("sam@home")
    sam.get_by_label("Your name").fill("Sam")
    sam.get_by_label("PIN (4–8 digits)").fill("0000")
    sam.get_by_label("PIN again").fill("0000")
    sam.get_by_role("button", name="Make account").click()
    sam.get_by_text("Needs your answer").wait_for()
    sam.get_by_role("link", name="Saturday game night").click()
    sam.get_by_role("radio", name="Maybe").click()
    sam.get_by_role("radio", name="If needed").first.click()
    sam.get_by_role("radio", name="Yes").nth(1).click()
    sam.get_by_role("checkbox", name="Catan", exact=True).check()
    sam.get_by_role("checkbox", name="Wingspan", exact=True).check()
    sam.get_by_label("Note").fill("only after 3")
    sam.get_by_label("Note").press("Tab")  # the note saves when you leave it
    sam.wait_for_timeout(800)
    sam.get_by_text("Saved ✓").wait_for()
    sam.screenshot(path=f"{shots}/s3-plan-guest.png", full_page=True)
    print("guest sees host controls:", sam.get_by_role("heading", name="Host").count())

    # Dad locks in the first time with Catan.
    dad.goto(plan_url)
    dad.locator(".people strong", has_text="Sam").wait_for()
    dad.locator("input[name=lock-time]").first.check()
    dad.get_by_role("checkbox", name=re.compile(r"^Catan \(")).check()
    dad.get_by_role("button", name="Lock it in").click()
    dad.get_by_text("on!").wait_for()
    dad.screenshot(path=f"{shots}/s3-plan-locked.png", full_page=True)
    dad.get_by_role("link", name="Home", exact=True).click()
    dad.get_by_text("Coming up").wait_for()
    dad.screenshot(path=f"{shots}/s3-home.png", full_page=True)
    print("coming up:", dad.locator(".plans li").first.inner_text().replace("\n", " | "))

    # Sprint 4: Sam was told about the lock-in, and comments.
    sam.reload()
    bell = sam.get_by_role("link", name=re.compile(r"^Notifications, \d+ unread"))
    bell.wait_for(timeout=20_000)
    print("sam's bell:", bell.get_attribute("aria-label"))
    bell.click()
    sam.get_by_text("Dad locked in").wait_for()
    sam.screenshot(path=f"{shots}/s4-notifications.png", full_page=True)
    sam.get_by_role("link", name=re.compile("Dad locked in")).click()
    sam.get_by_label("Add a comment").fill("I'll bring the expansion")
    sam.get_by_role("button", name="Post").click()
    sam.locator(".comments .text", has_text="I'll bring the expansion").wait_for()
    dad.goto(plan_url)
    dad.locator(".comments .text", has_text="I'll bring the expansion").wait_for()
    dad.screenshot(path=f"{shots}/s4-comments.png", full_page=True)

    # Sprint 5: the plan download and Dad's feed.
    href = dad.get_by_role("link", name=re.compile("Add to my calendar")).get_attribute("href")
    print("plan file:", href)
    dad.get_by_role("link", name=re.compile(r"^Dad$")).click()
    dad.get_by_role("button", name="Get my calendar link").click()
    feed_url = dad.locator(".feed").inner_text().strip()
    dad.screenshot(path=f"{shots}/s5-profile.png", full_page=True)
    browser.close()

import urllib.request
from icalendar import Calendar

# The feed URL names the host the browser used; fetch it as a calendar app would.
with urllib.request.urlopen(feed_url) as r:
    feed = r.read()
cal = Calendar.from_ical(feed)
events = [c for c in cal.walk("VEVENT")]
print("feed:", len(events), "event(s):", [(str(e.get("SUMMARY")), str(e.get("STATUS")), e.decoded("DTSTART").isoformat()) for e in events])
assert events and str(events[0].get("STATUS")) == "CONFIRMED"
print("console errors:", errors)
