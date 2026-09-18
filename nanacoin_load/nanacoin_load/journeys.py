import os
import threading
import uuid
from concurrent.futures import ThreadPoolExecutor

from .client import API
from .common import STATE, credentials, write_json


def prepare(host, evidence):
    api = API(host, evidence)
    status = api.call("GET", "/api/v1/status")
    username, password = credentials()
    if not status["provisioned"]:
        api.call(
            "POST",
            "/api/v1/provision",
            {
                "username": username,
                "password": password,
                "display_name": "Load Nana",
                "household_name": "Locust test household",
            },
            expected=201,
        )
    nana = api.login(username, password)
    users = api.call("GET", "/api/v1/users", token=nana["token"])["users"]
    members = []
    member_password = os.getenv("NANA_MEMBER_PASSWORD", "locust-test-pin")
    for name in ("load_alice", "load_bob"):
        if not any(u["username"] == name for u in users):
            api.call(
                "POST",
                "/api/v1/users",
                {"username": name, "display_name": name, "password": member_password},
                token=nana["token"],
                expected=201,
            )
        member = api.login(name, member_password)
        me = api.call("GET", "/api/v1/me", token=member["token"])
        if me["balance"] < 10000:
            api.call(
                "POST",
                "/api/v1/admin/issue",
                {"to": me["account"], "amount": 10000 - me["balance"], "reason": "Locust fixture funding"},
                token=nana["token"],
                expected=201,
                key=uuid.uuid4().hex,
            )
        members.append(member)
    fixture = {"host": host.rstrip("/"), "nana": nana, "members": members}
    STATE.parent.mkdir(exist_ok=True)
    write_json(STATE, fixture)
    evidence.emit("check", name="fixture prepared: Nana and two funded members", passed=True)
    return fixture


def e2e(host, fixture, evidence):
    api = API(host, evidence)
    nana, (alice, bob) = fixture["nana"], fixture["members"]
    at, bt, nt = alice["token"], bob["token"], nana["token"]
    aid, bid = alice["user"]["account"], bob["user"]["account"]

    def check(name, condition):
        evidence.emit("check", name=name, passed=bool(condition))
        if not condition:
            raise AssertionError(name)

    before = api.call("GET", "/api/v1/me", token=at)["balance"]
    tx = api.call(
        "POST",
        "/api/v1/transfers",
        {"to": bid, "amount": 7, "memo": "E2E transfer"},
        token=at,
        expected=201,
        key=uuid.uuid4().hex,
    )
    check("transfer debits exactly seven", api.call("GET", "/api/v1/me", token=at)["balance"] == before - 7)
    api.call(
        "POST",
        f"/api/v1/transactions/{tx['id']}/reverse",
        {"reason": "E2E correction"},
        token=nt,
        expected=201,
        key=uuid.uuid4().hex,
    )
    check("reversal restores balance", api.call("GET", "/api/v1/me", token=at)["balance"] == before)
    listing = api.call(
        "POST",
        "/api/v1/listings",
        {"title": "Locust E2E item", "description": "Disposable test listing", "price": 3},
        token=at,
        expected=201,
    )
    key = uuid.uuid4().hex
    purchase = api.call(
        "POST", f"/api/v1/listings/{listing['id']}/purchase", {}, token=bt, expected=201, key=key
    )
    replay = api.call(
        "POST", f"/api/v1/listings/{listing['id']}/purchase", {}, token=bt, expected=201, key=key
    )
    check(
        "purchase retry returns the original transaction",
        purchase["transaction"]["id"] == replay["transaction"]["id"],
    )
    check("purchase pays seller once", api.call("GET", "/api/v1/me", token=at)["balance"] == before + 3)
    # Each worker has its own TCP session; a barrier aligns the four attempts.
    before = api.call("GET", "/api/v1/me", token=bt)["balance"]
    key = uuid.uuid4().hex
    barrier = threading.Barrier(4, timeout=10)

    def issue(_):
        barrier.wait()
        return API(host, evidence).call(
            "POST",
            "/api/v1/admin/issue",
            {"to": bid, "amount": 1, "reason": "E2E concurrent replay"},
            token=nt,
            expected=201,
            key=key,
        )

    with ThreadPoolExecutor(max_workers=4) as pool:
        results = list(pool.map(issue, range(4)))
    check("four overlapping retries share one transaction ID", len({r["id"] for r in results}) == 1)
    check("four retries credit once", api.call("GET", "/api/v1/me", token=bt)["balance"] == before + 1)
    api.call("GET", "/api/v1/transactions", token=at, expected=403)
    check("member cannot read Nana's full ledger", True)
    history = api.call("GET", f"/api/v1/accounts/{aid}/transactions?limit=30", token=at)
    check("account history is readable", bool(history["transactions"]))
    status = api.call("GET", "/api/v1/status")
    check("ledger remains balanced", status["ledger_balanced"] is True)
    evidence.emit("snapshot", role="e2e", data=api.call("GET", "/api/v1/diag"))
