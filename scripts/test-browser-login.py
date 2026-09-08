"""Run with a local web server and Python Playwright installed."""
import os
from urllib.parse import parse_qs, urlparse
from playwright.sync_api import sync_playwright

with sync_playwright() as p:
    browser = p.chromium.launch(channel="chrome", headless=True)
    page = browser.new_page()
    origin = os.environ.get("SS_WEB_URL", "http://localhost:3000")
    page.route("**/api/me", lambda route: route.fulfill(status=401, json={"error": {"code": "unauthorized", "message": "Sign in"}}))
    page.route("**/api/auth/login?*", lambda route: route.fulfill(body="Login endpoint reached"))
    page.goto(origin)
    field = page.get_by_label("Organization ID")
    field.wait_for()
    assert field.input_value() == ""
    for invalid in ["", "ab", "Bad Org", "a" * 51]:
        field.fill(invalid)
        page.get_by_role("button", name="Log in with Silicon IAM").click()
        assert not field.evaluate("el => el.checkValidity()")
        assert urlparse(page.url).path == "/"
    field.fill("my-org")
    with page.expect_request("**/api/auth/login?*") as request:
        page.get_by_role("button", name="Log in with Silicon IAM").click()
    assert parse_qs(urlparse(request.value.url).query) == {"org": ["my-org"], "next": ["/o/my-org/tables"]}
    page.route("**/api/me", lambda route: route.fulfill(json={"id": "alice", "kind": "carbon", "org": "my-org", "tags": []}))
    page.goto(origin)
    page.wait_for_url("**/o/my-org/tables")
    assert page.get_by_label("Switch organization").count() == 0
    assert page.get_by_text("Choose an organization", exact=True).count() == 0
    browser.close()
    print("PASS: explicit org validation, server navigation, and session-bound home")
