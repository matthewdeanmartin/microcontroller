//! Non-secret deployment configuration, shared with the build script.
use miniframework::message;
use miniframework::wire::{self, Format};

message! {
    pub struct ScrapeTarget {
        1 name: String,
        2 url: String,
        3 every_s: u32,
    }
}

message! {
    pub struct Deployment {
        1 targets: Vec<ScrapeTarget>,
    }
}

pub fn parse(bytes: &[u8]) -> Result<Deployment, String> {
    if bytes.len() > 16 * 1024 {
        return Err("scrape configuration exceeds 16 KiB".into());
    }
    let config: Deployment = wire::decode(Format::Json, bytes).map_err(|e| e.to_string())?;
    if config.targets.len() > 16 {
        return Err("at most 16 configured targets".into());
    }
    let mut names = std::collections::HashSet::new();
    let mut urls = std::collections::HashSet::new();
    for target in &config.targets {
        if target.name.is_empty()
            || target.name.len() > 64
            || !target
                .name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        {
            return Err("target names need 1–64 letters, digits, underscores or hyphens".into());
        }
        let url = miniframework::fetch::split_url(&target.url).map_err(|e| e.to_string())?;
        if url.1.contains('@') || target.url.len() > 512 || target.url.contains(['\n', '\r']) {
            return Err("scrape URLs cannot contain credentials or line breaks".into());
        }
        if !(5..=3600).contains(&target.every_s) {
            return Err("every_s must be between 5 and 3600".into());
        }
        if !names.insert(&target.name) || !urls.insert(&target.url) {
            return Err("duplicate target name or URL".into());
        }
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_household_file_and_rejects_invalid_and_duplicate_targets() {
        assert_eq!(
            parse(include_bytes!("../config/scrapes.json"))
                .unwrap()
                .targets
                .len(),
            4
        );
        for bad in [
            r#"{"targets":[{"name":"peer","url":"ftp://peer/metrics","every_s":30}]}"#,
            r#"{"targets":[{"name":"peer","url":"http://peer/metrics","every_s":0}]}"#,
            r#"{"targets":[{"name":"peer","url":"http://u:p@peer/metrics","every_s":30}]}"#,
            r#"{"targets":[{"name":"peer","url":"http://peer/metrics","every_s":30},{"name":"peer","url":"http://other/metrics","every_s":30}]}"#,
        ] {
            assert!(parse(bad.as_bytes()).is_err());
        }
    }
}
