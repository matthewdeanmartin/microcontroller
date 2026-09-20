//! Count only this test thread, so the Rust test harness doesn't pollute counts.
mod common;
use nanacoin::{api, domain::*, journal::*};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

struct Counting;
thread_local! { static ENABLED: Cell<bool> = const { Cell::new(false) }; static COUNT: Cell<usize> = const { Cell::new(0) }; }
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ENABLED.with(|enabled| {
            if enabled.get() {
                COUNT.with(|count| count.set(count.get() + 1));
            }
        });
        // SAFETY: forwards the caller's allocator contract unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: forwards the caller's allocator contract unchanged.
        unsafe { System.dealloc(pointer, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;
struct Preallocated(Vec<[u8; FRAME_SIZE]>);
impl Journal for Preallocated {
    fn read(&mut self, index: usize, frame: &mut [u8; FRAME_SIZE]) -> Result<bool, Error> {
        match self.0.get(index) {
            Some(data) => {
                *frame = *data;
                Ok(true)
            }
            None => Ok(false),
        }
    }
    fn append(&mut self, _: usize, frame: &[u8; FRAME_SIZE]) -> Result<(), Error> {
        self.0.push(*frame);
        Ok(())
    }
}

#[test]
fn forex_recycling_and_repeated_http_trades_do_not_allocate() {
    use core::fmt::Write;
    use nanacoin::{auth::PasswordVerifier, forex::QuoteSide};
    let mut s = Service::open(Preallocated(Vec::with_capacity(2100))).unwrap();
    common::provision(&mut s);
    s.execute(
        MemberId(1),
        2,
        Command::CreateMember {
            username: Name::try_from("bob").unwrap(),
            display_name: Name::try_from("Bob").unwrap(),
            password: PasswordVerifier::hash("1234").unwrap(),
            role: Role::User,
            grant: 100,
        },
    )
    .unwrap();
    s.execute(
        MemberId(1),
        3,
        Command::Issue {
            to: MemberId(1),
            amount: 100,
            memo: Memo::new(),
        },
    )
    .unwrap();
    for (request, member) in [(4, 1), (5, 2)] {
        s.execute(
            MemberId(1),
            request,
            Command::IssueUsd {
                to: MemberId(member),
                cents: 1000,
                memo: Memo::new(),
            },
        )
        .unwrap();
    }
    let bob = common::login_as(&mut s, "bob", "1234");
    let mut output = vec![0; api::RESPONSE_LIMIT];
    COUNT.with(|c| c.set(0));
    ENABLED.with(|e| e.set(true));
    for i in 0..1000 {
        let receipt = s
            .execute(
                MemberId(1),
                6 + i,
                Command::PostQuote {
                    side: if i % 2 == 0 {
                        QuoteSide::ASK
                    } else {
                        QuoteSide::BID
                    },
                    cents_per_coin: 25,
                    coins: 1,
                    expires_at: 0,
                },
            )
            .unwrap();
        let mut path = heapless::String::<80>::new();
        write!(path, "/api/v1/quotes/quote-{}/take", receipt.sequence).unwrap();
        assert_eq!(
            api::handle_keyed(&mut s, "POST", &path, &bob, &path, b"{}", &mut output).0,
            201
        );
        assert_eq!(
            api::handle_keyed(&mut s, "POST", &path, &bob, &path, b"{}", &mut output).0,
            201
        );
        assert_eq!(
            api::handle(&mut s, "GET", "/api/v1/quotes", &bob, b"", &mut output).0,
            200
        );
    }
    ENABLED.with(|e| e.set(false));
    assert_eq!(COUNT.with(Cell::get), 0);
    assert_eq!(s.state().members[0].balance, 100);
    assert_eq!(s.state().members[0].usd_cents, 1000);
    assert_eq!(s.state().history.len(), HISTORY);
    assert_eq!(s.state().transactions, 2004);
    assert_eq!(s.state().quotes.len(), nanacoin::forex::QUOTES);
    s.state().check_invariants().unwrap();
}

#[test]
fn full_offer_table_and_repeated_offer_http_requests_do_not_allocate() {
    use nanacoin::{auth::PasswordVerifier, offers::*};
    let mut s = Service::open(Preallocated(Vec::with_capacity(200))).unwrap();
    common::provision(&mut s);
    s.execute(
        MemberId(1),
        2,
        Command::CreateMember {
            username: Name::try_from("bob").unwrap(),
            display_name: Name::try_from("Bob").unwrap(),
            password: PasswordVerifier::hash("1234").unwrap(),
            role: Role::User,
            grant: 100,
        },
    )
    .unwrap();
    let listing = s
        .execute(
            MemberId(1),
            3,
            Command::List {
                title: Title::try_from("Cookies").unwrap(),
                description: Memo::new(),
                price: 20,
                side: Side::Sell,
                details: None,
            },
        )
        .unwrap()
        .sequence;
    let nana = common::login(&mut s);
    let bob = common::login_as(&mut s, "bob", "1234");
    let mut output = vec![0; api::RESPONSE_LIMIT];
    let make_path = format!("/api/v1/listings/listing-{listing}/offers");
    let accept_path = format!("/api/v1/offers/offer-{}/accept", listing + 1);
    let undo_path = format!("/api/v1/offers/offer-{}/unaccept", listing + 1);
    COUNT.with(|c| c.set(0));
    ENABLED.with(|e| e.set(true));
    for _ in 0..OFFERS {
        assert_eq!(
            api::handle(
                &mut s,
                "POST",
                &make_path,
                &bob,
                br#"{"amount":15,"message":"Saturday"}"#,
                &mut output
            )
            .0,
            201
        );
    }
    for _ in 0..1000 {
        assert_eq!(
            api::handle(&mut s, "GET", "/api/v1/offers", &nana, b"", &mut output).0,
            200
        );
    }
    assert_eq!(
        api::handle_keyed(
            &mut s,
            "POST",
            &accept_path,
            &nana,
            "accept",
            b"{}",
            &mut output
        )
        .0,
        201
    );
    assert_eq!(
        api::handle_keyed(
            &mut s,
            "POST",
            &undo_path,
            &bob,
            "undo",
            br#"{"reason":"Not delivered"}"#,
            &mut output
        )
        .0,
        200
    );
    for _ in 0..1000 {
        assert_eq!(
            api::handle_keyed(
                &mut s,
                "POST",
                &accept_path,
                &nana,
                "accept",
                b"{}",
                &mut output
            )
            .0,
            201
        );
    }
    ENABLED.with(|e| e.set(false));
    assert_eq!(COUNT.with(Cell::get), 0);
    assert_eq!(s.state().member(MemberId(2)).unwrap().balance, 100);
}

#[test]
fn command_and_state_serialization_do_not_allocate_after_startup() {
    let mut service = Service::open(Preallocated(Vec::with_capacity(3001))).unwrap();
    common::provision(&mut service);
    let mut output = vec![0; api::RESPONSE_LIMIT];
    let auth = common::login(&mut service);
    ENABLED.with(|enabled| enabled.set(true));
    for request_id in 2..3000 {
        service
            .execute(
                MemberId(1),
                request_id,
                Command::Issue {
                    to: MemberId(1),
                    amount: 1,
                    memo: Memo::try_from("🍪 \"cookies\"").unwrap(),
                },
            )
            .unwrap();
        let (status, _) = api::handle(
            &mut service,
            "GET",
            "/api/v1/state",
            &auth,
            b"",
            &mut output,
        );
        assert_eq!(status, 200);
    }
    let body =
        br#"{"request_id":3000,"command":{"issue":{"to":1,"amount":1,"memo":"escaped\ntext"}}}"#;
    assert_eq!(
        api::handle(
            &mut service,
            "POST",
            "/api/v1/commands",
            &auth,
            body,
            &mut output
        )
        .0,
        200
    );
    ENABLED.with(|enabled| enabled.set(false));
    assert_eq!(COUNT.with(Cell::get), 0);
    assert_eq!(service.state().history.len(), HISTORY);
    assert_eq!(service.state().transactions, 2999);
    assert_eq!(service.state().member(MemberId(1)).unwrap().balance, 2999);
    println!(
        "State storage: {} bytes; response scratch: {} bytes",
        std::mem::size_of::<State>(),
        api::RESPONSE_LIMIT
    );
}
