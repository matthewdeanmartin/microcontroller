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
    let mut service = Service::open(Preallocated(Vec::with_capacity(200))).unwrap();
    common::provision(&mut service);
    let mut output = vec![0; api::RESPONSE_LIMIT];
    let auth = common::login(&mut service);
    ENABLED.with(|enabled| enabled.set(true));
    for request_id in 2..130 {
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
        br#"{"request_id":130,"command":{"issue":{"to":1,"amount":1,"memo":"escaped\ntext"}}}"#;
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
    println!(
        "State storage: {} bytes; response scratch: {} bytes",
        std::mem::size_of::<State>(),
        api::RESPONSE_LIMIT
    );
}
