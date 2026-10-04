//! Every message housemetrics sends or accepts. Tags are protobuf field
//! numbers and CBOR integer keys: add new fields with new numbers, never
//! renumber.
use miniframework::message;

message! {
    /// One series: a measurement, its tags and one field.
    pub struct SeriesInfo {
        1 id: u32,
        /// `measurement,tag=v field`, unique.
        2 key: String,
        3 measurement: String,
        4 tags: String,
        5 field: String,
        /// Raw points still held (older ones survive only as rollups).
        6 raw_points: u64,
        /// Oldest time with any data (raw or rollup), Unix ms.
        7 first_t: u64,
        8 last_t: u64,
        9 last_v: f64,
        /// Compressed raw bytes in use.
        10 raw_bytes: u32,
        /// Points ever accepted.
        11 total: u64,
        /// Oldest raw point, Unix ms.
        12 raw_from: u64,
    }
}

message! {
    /// How full the store is.
    pub struct StoreStats {
        1 series: u32,
        2 max_series: u32,
        3 blocks_used: u32,
        4 blocks_total: u32,
        5 block_bytes: u32,
        6 raw_points: u64,
        /// Compressed bytes per raw point, averaged over every series.
        7 bytes_per_point: f64,
        8 rollup_secs: u32,
        9 rollup_slots: u32,
        10 accepted: u64,
        11 rejected: u64,
        12 evicted_blocks: u64,
        /// Memory the store reserves when full.
        13 capacity_bytes: u32,
    }
}

message! {
    pub struct SeriesList {
        1 series: Vec<SeriesInfo>,
        2 store: StoreStats,
    }
}

message! {
    pub struct Point {
        /// Unix ms.
        1 t: u64,
        2 v: f64,
    }
}

message! {
    pub struct Bucket {
        /// Bucket start, Unix ms.
        1 t: u64,
        2 min: f64,
        3 max: f64,
        4 avg: f64,
        5 n: u32,
    }
}

message! {
    /// One series' data for a time range. `shape=rows` fills `points` or
    /// `buckets`; `shape=columns` fills `t0` + `dt` (deltas from the
    /// previous timestamp, the first from `t0`) and `v`, or
    /// `min`/`max`/`avg`/`n`.
    pub struct SeriesData {
        1 id: u32,
        2 key: String,
        /// "raw" or "buckets".
        3 kind: String,
        4 from: u64,
        5 to: u64,
        /// Bucket width in ms (0 for raw).
        6 step: u64,
        7 points: Vec<Point>,
        8 buckets: Vec<Bucket>,
        9 t0: u64,
        10 dt: Vec<u64>,
        11 v: Vec<f64>,
        12 min: Vec<f64>,
        13 max: Vec<f64>,
        14 avg: Vec<f64>,
        15 n: Vec<u32>,
        /// Raw paging: pass as `from` for the next page.
        16 next: Option<u64>,
    }
}

message! {
    pub struct QueryResult {
        1 series: Vec<SeriesData>,
    }
}

message! {
    /// One sample to write. `t` 0 means "now" (the board's clock).
    pub struct Sample {
        /// Measurement, e.g. `temp`.
        1 m: String,
        /// `room=attic,board=s2` (optional).
        2 tags: String,
        /// Field name; empty means `value`.
        3 f: String,
        /// Unix ms.
        4 t: u64,
        5 v: f64,
    }
}

message! {
    pub struct WriteBatch {
        1 samples: Vec<Sample>,
    }
}

message! {
    pub struct WriteResult {
        1 accepted: u32,
        2 rejected: u32,
        /// New series created by this write.
        3 created: u32,
        /// The first few problems, with line or sample numbers.
        4 errors: Vec<String>,
    }
}

message! {
    pub struct Device {
        1 id: u32,
        2 name: String,
        3 created: u64,
        /// First characters of the token, to tell tokens apart.
        4 prefix: String,
        5 last_seen: u64,
        6 writes: u32,
    }
}

message! {
    pub struct DeviceList {
        1 devices: Vec<Device>,
    }
}

message! {
    pub struct NewDevice {
        1 name: String,
    }
}

message! {
    /// The token is shown once; only its hash is kept.
    pub struct DeviceToken {
        1 device: Device,
        2 token: String,
    }
}

message! {
    pub struct Target {
        1 id: u32,
        2 name: String,
        /// http:// or https:// (household CA) URL returning Influx lines
        /// (text/plain) or JSON (numeric fields are flattened).
        3 url: String,
        4 every_s: u32,
        5 last_ok: u64,
        6 last_error: String,
        7 last_ms: u32,
        8 samples: u32,
    }
}

message! {
    pub struct TargetList {
        1 targets: Vec<Target>,
    }
}

message! {
    pub struct NewTarget {
        1 name: String,
        2 url: String,
        3 every_s: u32,
    }
}

message! {
    /// A synthetic record for format benchmarks: mixed types, like a
    /// typical page of app data.
    pub struct Row {
        1 id: u32,
        2 name: String,
        3 kind: String,
        4 value: f64,
        5 count: u32,
        6 ok: bool,
        7 t: u64,
    }
}

message! {
    pub struct Rows {
        1 rows: Vec<Row>,
    }
}

message! {
    /// The same rows, column by column.
    pub struct RowColumns {
        1 id: Vec<u32>,
        2 name: Vec<String>,
        3 kind: Vec<String>,
        4 value: Vec<f64>,
        5 count: Vec<u32>,
        6 ok: Vec<bool>,
        7 t: Vec<u64>,
    }
}

message! {
    /// Persisted device record (protobuf in NVS).
    pub struct StoredDevice {
        1 id: u32,
        2 name: String,
        3 created: u64,
        /// SHA-256 of the token, hex.
        4 hash: String,
        5 prefix: String,
    }
}

message! {
    pub struct StoredDevices {
        1 devices: Vec<StoredDevice>,
        2 next_id: u32,
    }
}

message! {
    pub struct StoredTargets {
        1 targets: Vec<NewTarget>,
        2 ids: Vec<u32>,
        3 next_id: u32,
        /// IDs owned by the embedded deployment configuration.
        4 managed_ids: Vec<u32>,
    }
}
