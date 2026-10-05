use super::*;

message! {
    /// A point.
    pub struct Pt {
        1 t: u64,
        2 v: f64,
    }
}

message! {
    pub struct Everything {
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
    }
}

fn sample() -> Everything {
    Everything {
        id: 300,
        name: "attic \"loft\" \\ é \n ✓".into(),
        delta: -123_456_789,
        ratio: 0.5,
        ok: true,
        points: (0..200)
            .map(|i| Pt {
                t: 1_727_900_000_000 + i * 10_000,
                v: 20.0 + i as f64 * 0.1,
            })
            .collect(),
        values: vec![0.0, -1.5, 1e300, 3.25],
        tags: vec!["a".into(), "".into(), "x".repeat(300)],
        maybe: Some(0),
        nested: Some(Pt { t: 7, v: -0.0 }),
        counts: vec![0, 1, 127, 128, 70_000],
    }
}

#[test]
fn every_format_round_trips() {
    let value = sample();
    for format in Format::ALL {
        let bytes = to_vec(format, &value).unwrap();
        let back: Everything = decode(format, &bytes).unwrap_or_else(|e| panic!("{format:?}: {e}"));
        assert_eq!(back, value, "{format:?}");
    }
}

#[test]
fn absent_and_empty_fields_round_trip() {
    let value = Everything::default();
    for format in Format::ALL {
        let bytes = to_vec(format, &value).unwrap();
        let back: Everything = decode(format, &bytes).unwrap();
        assert_eq!(back, value, "{format:?}");
    }
    // Proto3 writes nothing at all for an all-default message.
    assert!(to_vec(Format::Protobuf, &value).unwrap().is_empty());
}

#[test]
fn json_matches_serde_json() {
    let value = sample();
    let bytes = to_vec(Format::Json, &value).unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed["id"], 300);
    assert_eq!(parsed["name"], value.name.as_str());
    assert_eq!(parsed["delta"], -123_456_789);
    assert_eq!(parsed["ratio"], 0.5);
    assert_eq!(
        parsed["points"][199]["t"],
        1_727_900_000_000u64 + 199 * 10_000
    );
    assert_eq!(parsed["values"][2], 1e300);
    assert_eq!(parsed["nested"]["v"], -0.0);
    // And the reverse: serde_json's output decodes.
    let reencoded = serde_json::to_vec(&parsed).unwrap();
    let back: Everything = decode(Format::Json, &reencoded).unwrap();
    assert_eq!(back, value);
}

#[test]
fn json_whole_floats_print_like_javascript() {
    let bytes = to_vec(Format::Json, &Pt { t: 1, v: 21.0 }).unwrap();
    assert_eq!(std::str::from_utf8(&bytes).unwrap(), r#"{"t":1,"v":21}"#);
}

#[test]
fn cbor_matches_ciborium() {
    let value = sample();
    for (format, int_keys) in [(Format::Cbor, false), (Format::CborInt, true)] {
        let bytes = to_vec(format, &value).unwrap();
        let parsed: ciborium::Value = ciborium::from_reader(&bytes[..]).unwrap();
        let map = parsed.as_map().unwrap();
        assert_eq!(map.len(), 11);
        let (key, id) = &map[0];
        if int_keys {
            assert_eq!(key.as_integer().unwrap(), 1.into());
        } else {
            assert_eq!(key.as_text().unwrap(), "id");
        }
        assert_eq!(id.as_integer().unwrap(), 300.into());
        assert_eq!(map[2].1.as_integer().unwrap(), (-123_456_789).into());
        let points = map[5].1.as_array().unwrap();
        assert_eq!(points.len(), 200);
        // ciborium output decodes too.
        let mut again = Vec::new();
        ciborium::into_writer(&parsed, &mut again).unwrap();
        let back: Everything = decode(format, &again).unwrap();
        assert_eq!(back, value);
    }
}

#[test]
fn msgpack_matches_rmpv() {
    let value = sample();
    let bytes = to_vec(Format::MsgPack, &value).unwrap();
    let parsed = rmpv::decode::read_value(&mut &bytes[..]).unwrap();
    let map = parsed.as_map().unwrap();
    assert_eq!(map[0].0.as_str(), Some("id"));
    assert_eq!(map[0].1.as_u64(), Some(300));
    assert_eq!(map[2].1.as_i64(), Some(-123_456_789));
    assert_eq!(map[1].1.as_str(), Some(value.name.as_str()));
    let mut again = Vec::new();
    rmpv::encode::write_value(&mut again, &parsed).unwrap();
    let back: Everything = decode(Format::MsgPack, &again).unwrap();
    assert_eq!(back, value);
}

#[test]
fn protobuf_bytes_match_the_spec() {
    // From the protobuf encoding guide: field 1 = 150 -> 08 96 01.
    message! { pub struct Test1 { 1 a: u32 } }
    assert_eq!(
        to_vec(Format::Protobuf, &Test1 { a: 150 }).unwrap(),
        [0x08, 0x96, 0x01]
    );
    // Field 2 string "testing" -> 12 07 74 65 73 74 69 6e 67.
    message! { pub struct Test2 { 2 b: String } }
    assert_eq!(
        to_vec(
            Format::Protobuf,
            &Test2 {
                b: "testing".into()
            }
        )
        .unwrap(),
        b"\x12\x07testing"
    );
    // Field 3 embedded Test1{150} -> 1a 03 08 96 01.
    message! { pub struct Test3 { 3 c: Option<Test1> } }
    assert_eq!(
        to_vec(
            Format::Protobuf,
            &Test3 {
                c: Some(Test1 { a: 150 })
            }
        )
        .unwrap(),
        [0x1a, 0x03, 0x08, 0x96, 0x01]
    );
    // Packed repeated field 4 [3, 270, 86942] -> 22 06 03 8e 02 9e a7 05.
    message! { pub struct Test4 { 4 d: Vec<u32> } }
    assert_eq!(
        to_vec(
            Format::Protobuf,
            &Test4 {
                d: vec![3, 270, 86942]
            }
        )
        .unwrap(),
        [0x22, 0x06, 0x03, 0x8e, 0x02, 0x9e, 0xa7, 0x05]
    );
    // sint64 zigzag: -1 -> 1, 1 -> 2.
    message! { pub struct Test5 { 1 s: i64 } }
    assert_eq!(
        to_vec(Format::Protobuf, &Test5 { s: -1 }).unwrap(),
        [0x08, 0x01]
    );
    assert_eq!(
        to_vec(Format::Protobuf, &Test5 { s: 1 }).unwrap(),
        [0x08, 0x02]
    );
}

#[test]
fn protobuf_long_submessages_get_multibyte_lengths() {
    message! { pub struct Blob { 1 s: String } }
    message! { pub struct Outer { 1 inner: Option<Blob>, 2 after: u32 } }
    let value = Outer {
        inner: Some(Blob {
            s: "x".repeat(20_000),
        }),
        after: 9,
    };
    let bytes = to_vec(Format::Protobuf, &value).unwrap();
    // 0a, len(1 + 3 + 20000 = 20004) as a 3-byte varint, then the Blob.
    assert_eq!(&bytes[..4], &[0x0a, 0xa4, 0x9c, 0x01]);
    assert_eq!(decode::<Outer>(Format::Protobuf, &bytes).unwrap(), value);
}

#[test]
fn protobuf_accepts_unpacked_repeated_scalars() {
    message! { pub struct Test4 { 4 d: Vec<u32> } }
    // Older encoders write each element with its own key.
    let bytes = [0x20, 0x03, 0x20, 0x8e, 0x02];
    assert_eq!(
        decode::<Test4>(Format::Protobuf, &bytes).unwrap().d,
        vec![3, 270]
    );
}

#[test]
fn unknown_fields_are_skipped() {
    message! { pub struct Small { 1 t: u64 } }
    let value = sample();
    for format in Format::ALL {
        let bytes = to_vec(format, &value).unwrap();
        // Field 1 of Everything is `id` (u32), read as `t`.
        let small: Small = decode(format, &bytes).unwrap();
        if matches!(format, Format::Protobuf | Format::CborInt) {
            assert_eq!(small.t, 300, "{format:?}");
        } else {
            assert_eq!(small.t, 0, "{format:?}: names differ, so nothing matches");
        }
    }
}

#[test]
fn limit_overflows_cleanly() {
    let value = sample();
    for format in Format::ALL {
        let mut out = Vec::new();
        assert_eq!(encode(format, &value, &mut out, 64), Err(Error::Overflow));
        assert!(out.len() <= 64);
    }
}

#[test]
fn hostile_input_is_rejected_not_panicked() {
    let value = sample();
    for format in Format::ALL {
        let bytes = to_vec(format, &value).unwrap();
        for end in 0..bytes.len() {
            let _ = decode::<Everything>(format, &bytes[..end]);
        }
        let mut flipped = bytes.clone();
        for i in (0..flipped.len()).step_by(13) {
            flipped[i] ^= 0x5a;
            let _ = decode::<Everything>(format, &flipped);
        }
    }
    // Deep nesting is bounded.
    let deep = "[".repeat(10_000);
    let json = format!("{{\"x\":{deep}");
    assert!(decode::<Pt>(Format::Json, json.as_bytes()).is_err());
    let deep_cbor = vec![0x81u8; 10_000];
    let mut cbor = vec![0xa1, 0x61, b'x'];
    cbor.extend_from_slice(&deep_cbor);
    assert!(decode::<Pt>(Format::Cbor, &cbor).is_err());
    let mut mp = vec![0x81, 0xa1, b'x'];
    mp.extend(std::iter::repeat_n(0x91u8, 10_000));
    assert!(decode::<Pt>(Format::MsgPack, &mp).is_err());
}

#[test]
fn all_single_byte_inputs_and_each_bit_mutation_are_panic_free() {
    let value = Everything {
        id: 1,
        name: "é".into(),
        nested: Some(Pt { t: 7, v: -1.5 }),
        ..Default::default()
    };
    for format in Format::ALL {
        for byte in 0..=u8::MAX {
            let _ = decode::<Everything>(format, &[byte]);
        }
        let bytes = to_vec(format, &value).unwrap();
        for at in 0..bytes.len() {
            for bit in 0..8 {
                let mut mutated = bytes.clone();
                mutated[at] ^= 1 << bit;
                let _ = decode::<Everything>(format, &mutated);
            }
        }
    }
}

#[test]
fn negotiation() {
    assert_eq!(negotiate("", None).unwrap(), Format::Json);
    assert_eq!(negotiate("*/*", None).unwrap(), Format::Json);
    assert_eq!(
        negotiate("application/json, application/cbor;q=0.9", None).unwrap(),
        Format::Json
    );
    assert_eq!(
        negotiate("application/json;q=0.5, application/cbor; keys=int", None).unwrap(),
        Format::CborInt
    );
    assert_eq!(
        negotiate("application/x-protobuf", None).unwrap(),
        Format::Protobuf
    );
    assert_eq!(
        negotiate("text/html", Some("msgpack")).unwrap(),
        Format::MsgPack
    );
    assert!(negotiate("", Some("xml")).is_err());
    for f in Format::ALL {
        assert_eq!(Format::from_mime(f.mime()), Some(f));
        assert_eq!(Format::from_name(f.name()), Some(f));
    }
}

#[test]
fn schema_and_proto_text() {
    let doc = schema_doc(&[Everything::SCHEMA]);
    let names: Vec<_> = doc.messages.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(
        names,
        ["Everything", "Pt", "SchemaDoc", "MessageDoc", "FieldDoc"]
    );
    let everything = &doc.messages[0];
    assert_eq!(everything.fields[5].kind, "message");
    assert_eq!(everything.fields[5].message, "Pt");
    assert!(everything.fields[5].repeated);
    assert!(everything.fields[8].optional);
    let proto = proto_text("test", &[Everything::SCHEMA]);
    assert!(proto.contains("message Everything {"));
    assert!(proto.contains("  repeated Pt points = 6;"));
    assert!(proto.contains("  optional uint64 maybe = 9;"));
    assert!(proto.contains("  sint64 delta = 3;"));
    // The schema document itself round-trips in every format.
    for format in Format::ALL {
        let bytes = to_vec(format, &doc).unwrap();
        assert_eq!(decode::<SchemaDoc>(format, &bytes).unwrap(), doc);
    }
}

use super::schema::SchemaDoc;

#[cfg(feature = "gzip")]
#[test]
fn gzip_round_trip_and_python_compatible_header() {
    let input = to_vec(Format::Json, &sample()).unwrap();
    let mut out = Vec::new();
    gzip::compress(&input, 6, &mut out);
    assert!(out.len() < input.len() / 2);
    assert_eq!(gzip::decompress(&out, 1 << 20).unwrap(), input);
    assert_eq!(gzip::decompress(&out, 100), Err(Error::Overflow));
    let at = out.len() - 6;
    out[at] ^= 1;
    assert!(gzip::decompress(&out, 1 << 20).is_err());
}
