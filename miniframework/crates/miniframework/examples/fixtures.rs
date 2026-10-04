//! Writes one message in every wire format, plus the schema, for the
//! TypeScript decoder tests: `cargo run --example fixtures -- <dir>`.
use miniframework::message;
use miniframework::wire::{self, Format, Message};

message! {
    pub struct Pt {
        1 t: u64,
        2 v: f64,
    }
}

message! {
    /// Covers every field kind the decoders must handle.
    pub struct Fixture {
        1 id: u32,
        2 name: String,
        3 delta: i64,
        4 ratio: f32,
        5 ok: bool,
        6 points: Vec<Pt>,
        7 values: Vec<f64>,
        8 tags: Vec<String>,
        9 maybe: Option<u64>,
        10 nested: Option<Pt>,
        11 counts: Vec<u32>,
        12 missing: Option<Pt>,
        13 off: bool,
        14 zero: u64,
        15 deltas: Vec<i64>,
    }
}

fn main() -> std::io::Result<()> {
    let dir = std::env::args().nth(1).expect("usage: fixtures <dir>");
    std::fs::create_dir_all(&dir)?;
    let value = Fixture {
        id: 300,
        name: "attic \"loft\" é ✓ \u{1F321}".into(),
        delta: -123_456_789,
        ratio: 0.5,
        ok: true,
        points: (0..50)
            .map(|i| Pt {
                t: 1_727_900_000_000 + i * 10_000,
                v: 20.0 + i as f64 * 0.1,
            })
            .collect(),
        values: vec![0.0, -1.5, 1e300, 3.25, -0.0],
        tags: vec!["a".into(), "".into(), "x".repeat(300)],
        maybe: Some(0),
        nested: Some(Pt { t: 7, v: 21.5 }),
        counts: vec![0, 1, 127, 128, 70_000, u32::MAX],
        missing: None,
        off: false,
        zero: 0,
        deltas: vec![-1, 1, -70_000, i64::from(i32::MIN), 9_007_199_254_740_991],
    };
    for format in Format::ALL {
        let bytes = wire::to_vec(format, &value).unwrap();
        std::fs::write(format!("{dir}/fixture.{}.bin", format.name()), bytes)?;
    }
    let schema = wire::schema_doc(&[Fixture::SCHEMA]);
    std::fs::write(
        format!("{dir}/schema.json"),
        wire::to_vec(Format::Json, &schema).unwrap(),
    )?;
    println!("wrote fixtures to {dir}");
    Ok(())
}
