//! Who may do what.
//!
//! - Reading metrics: anyone on the network (it is a household dashboard).
//! - Writing metrics: a device token (`Authorization: Bearer hm_...`), or
//!   the admin password.
//! - Managing devices, scrape targets and series: the admin password, and
//!   on the board only over HTTPS (a password must not cross the LAN in
//!   clear text).
//!
//! Tokens are stored as SHA-256 only, in the key-value store (NVS on the
//! board), so they survive reboots even though metrics do not.
use crate::messages::{Device, StoredDevice, StoredDevices};
use miniframework::kv::Kv;
use miniframework::wire::{self, Format};
use miniframework::{ApiError, Request};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Constant-time comparison (no early exit on the first difference).
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn bearer<'a>(req: &Request<'a>) -> Option<&'a str> {
    let header = req.header("Authorization").trim();
    let (scheme, token) = header.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

pub struct Admin {
    hash: [u8; 32],
    /// Desktop development: allow admin over plain HTTP.
    pub allow_http: bool,
}

impl Admin {
    pub fn new(password: &str, allow_http: bool) -> Self {
        Self {
            hash: sha256(password.as_bytes()),
            allow_http,
        }
    }

    pub fn matches(&self, secret: &str) -> bool {
        same(&sha256(secret.as_bytes()), &self.hash)
    }

    pub fn check(&self, req: &Request<'_>) -> Result<(), ApiError> {
        if !req.secure && !self.allow_http {
            return Err(ApiError::new(
                403,
                "https_required",
                "Admin requests must use HTTPS so the password is not sent in clear text",
            ));
        }
        match bearer(req) {
            Some(secret) if self.matches(secret) => Ok(()),
            Some(_) => Err(ApiError::unauthorized("Wrong admin password")),
            None => Err(ApiError::unauthorized(
                "Send Authorization: Bearer <admin password>",
            )),
        }
    }
}

struct Record {
    stored: StoredDevice,
    hash: [u8; 32],
    last_seen: u64,
    writes: u32,
}

pub struct Devices {
    kv: Arc<dyn Kv>,
    inner: Mutex<(Vec<Record>, u32)>,
}

const KEY: &str = "devices";

pub enum Writer {
    Admin,
    Device(u32),
}

impl Devices {
    pub fn load(kv: Arc<dyn Kv>) -> Self {
        let stored: StoredDevices = kv
            .get(KEY)
            .and_then(|bytes| wire::decode(Format::Protobuf, &bytes).ok())
            .unwrap_or_default();
        let records = stored
            .devices
            .into_iter()
            .filter_map(|d| {
                let mut hash = [0u8; 32];
                for (i, chunk) in d.hash.as_bytes().chunks(2).enumerate().take(32) {
                    hash[i] = u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
                }
                Some(Record {
                    stored: d,
                    hash,
                    last_seen: 0,
                    writes: 0,
                })
            })
            .collect();
        Self {
            kv,
            inner: Mutex::new((records, stored.next_id.max(1))),
        }
    }

    fn save(&self, records: &[Record], next_id: u32) -> Result<(), ApiError> {
        let stored = StoredDevices {
            devices: records.iter().map(|r| r.stored.clone()).collect(),
            next_id,
        };
        let bytes = wire::to_vec(Format::Protobuf, &stored).map_err(ApiError::from)?;
        self.kv
            .set(KEY, &bytes)
            .map_err(|e| ApiError::new(500, "storage_failed", e.to_string()))
    }

    fn view(r: &Record) -> Device {
        Device {
            id: r.stored.id,
            name: r.stored.name.clone(),
            created: r.stored.created,
            prefix: r.stored.prefix.clone(),
            last_seen: r.last_seen,
            writes: r.writes,
        }
    }

    pub fn list(&self) -> Vec<Device> {
        self.inner
            .lock()
            .unwrap()
            .0
            .iter()
            .map(Self::view)
            .collect()
    }

    /// Creates a device and returns it with its one-time token.
    pub fn create(&self, name: &str, now: u64) -> Result<(Device, String), ApiError> {
        let name = name.trim();
        if name.is_empty() || name.len() > 40 {
            return Err(ApiError::bad_request("Name the device (1-40 characters)"));
        }
        let mut random = [0u8; 16];
        getrandom::getrandom(&mut random)
            .map_err(|_| ApiError::new(500, "no_randomness", "No random source"))?;
        let token = format!("hm_{}", hex(&random));
        let mut inner = self.inner.lock().unwrap();
        if inner.0.len() >= 64 {
            return Err(ApiError::new(409, "too_many_devices", "At most 64 devices"));
        }
        if inner.0.iter().any(|r| r.stored.name == name) {
            return Err(ApiError::new(
                409,
                "duplicate_name",
                "A device with that name exists",
            ));
        }
        let id = inner.1;
        let hash = sha256(token.as_bytes());
        inner.0.push(Record {
            stored: StoredDevice {
                id,
                name: name.into(),
                created: now,
                hash: hex(&hash),
                prefix: token[..7].into(),
            },
            hash,
            last_seen: 0,
            writes: 0,
        });
        inner.1 += 1;
        let (records, next) = &*inner;
        if let Err(e) = self.save(records, *next) {
            inner.0.pop();
            inner.1 -= 1;
            return Err(e);
        }
        Ok((Self::view(inner.0.last().unwrap()), token))
    }

    pub fn remove(&self, id: u32) -> Result<bool, ApiError> {
        let mut inner = self.inner.lock().unwrap();
        let before = inner.0.len();
        inner.0.retain(|r| r.stored.id != id);
        if inner.0.len() == before {
            return Ok(false);
        }
        let (records, next) = &*inner;
        self.save(records, *next)?;
        Ok(true)
    }

    /// Who is writing: a device token, or the admin password.
    pub fn writer(&self, req: &Request<'_>, admin: &Admin, now: u64) -> Result<Writer, ApiError> {
        let token = bearer(req).ok_or_else(|| {
            ApiError::unauthorized("Writing needs Authorization: Bearer <device token>")
        })?;
        let hash = sha256(token.as_bytes());
        let mut inner = self.inner.lock().unwrap();
        // Check every record (constant time per record, no early exit).
        let mut found = None;
        for (i, r) in inner.0.iter().enumerate() {
            if same(&r.hash, &hash) {
                found = Some(i);
            }
        }
        if let Some(i) = found {
            let r = &mut inner.0[i];
            r.last_seen = now;
            r.writes += 1;
            return Ok(Writer::Device(r.stored.id));
        }
        drop(inner);
        if admin.matches(token) && (req.secure || admin.allow_http) {
            return Ok(Writer::Admin);
        }
        Err(ApiError::unauthorized("Unknown device token"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniframework::kv::MemKv;

    fn req<'a>(headers: &'a [(String, String)]) -> Request<'a> {
        Request::new("POST", "/api/v1/write", headers, b"", true)
    }

    #[test]
    fn tokens_survive_reload_and_revocation_sticks() {
        let kv: Arc<dyn Kv> = Arc::new(MemKv::default());
        let admin = Admin::new("secret", false);
        let devices = Devices::load(kv.clone());
        let (device, token) = devices.create("attic sensor", 5).unwrap();
        assert!(token.starts_with("hm_") && token.len() == 35);
        assert_eq!(device.prefix, &token[..7]);
        assert!(devices.create("attic sensor", 5).is_err());

        let reloaded = Devices::load(kv.clone());
        let headers = vec![("Authorization".to_string(), format!("Bearer {token}"))];
        assert!(matches!(
            reloaded.writer(&req(&headers), &admin, 9),
            Ok(Writer::Device(1))
        ));
        assert_eq!(reloaded.list()[0].writes, 1);

        let bad = vec![("Authorization".to_string(), "Bearer hm_nope".to_string())];
        assert!(reloaded.writer(&req(&bad), &admin, 9).is_err());
        let admin_headers = vec![("Authorization".to_string(), "Bearer secret".to_string())];
        assert!(matches!(
            reloaded.writer(&req(&admin_headers), &admin, 9),
            Ok(Writer::Admin)
        ));

        assert!(reloaded.remove(device.id).unwrap());
        let again = Devices::load(kv);
        assert!(again.writer(&req(&headers), &admin, 9).is_err());
    }

    #[test]
    fn admin_requires_https_on_the_board() {
        let admin = Admin::new("secret", false);
        let headers = vec![("Authorization".to_string(), "Bearer secret".to_string())];
        let plain = Request::new("GET", "/api/v1/devices", &headers, b"", false);
        assert_eq!(admin.check(&plain).unwrap_err().code, "https_required");
        let tls = Request::new("GET", "/api/v1/devices", &headers, b"", true);
        assert!(admin.check(&tls).is_ok());
        let wrong = vec![("Authorization".to_string(), "Bearer nope".to_string())];
        let tls = Request::new("GET", "/api/v1/devices", &wrong, b"", true);
        assert_eq!(admin.check(&tls).unwrap_err().status, 401);
    }
}
