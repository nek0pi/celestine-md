//! Background check against GitHub Releases for a newer *feature* line.
//!
//! Version policy (CelestineMD):
//! - `MAJOR.MINOR.PATCH` (e.g. `0.2.0`)
//! - Notify when `MAJOR` or `MINOR` increases (feature / "major" releases)
//! - Do **not** notify for `PATCH`-only bumps (bugfixes / small improvements)

use serde::Deserialize;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

pub const RELEASES_PAGE: &str = "https://github.com/nek0pi/celestine-md/releases";

const API_LATEST: &str = "https://api.github.com/repos/nek0pi/celestine-md/releases/latest";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SemVer {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl SemVer {
    pub fn parse(raw: &str) -> Option<Self> {
        let s = raw.trim().trim_start_matches('v');
        let mut parts = s.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next().unwrap_or("0").parse().ok()?;
        // Reject trailing junk like "1.2.3-beta" (split already took patch token).
        if s.contains('-') || s.contains('+') {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
        })
    }

    pub fn feature_line(self) -> (u64, u64) {
        (self.major, self.minor)
    }

    pub fn display(self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// True when `latest` is a newer feature line than `current` (ignores patch).
pub fn should_notify(current: SemVer, latest: SemVer) -> bool {
    latest.feature_line() > current.feature_line()
}

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub version: SemVer,
    pub html_url: String,
}

enum CheckMsg {
    Available(UpdateInfo),
    None,
}

pub struct UpdateChecker {
    rx: Receiver<CheckMsg>,
    /// Set once we know whether an update banner should show.
    available: Option<UpdateInfo>,
    settled: bool,
}

impl UpdateChecker {
    /// Spawn a background fetch. No-op when `CELESTINE_NO_UPDATE` is set.
    pub fn start(current_version: &str) -> Self {
        let (tx, rx) = mpsc::channel();
        let current = SemVer::parse(current_version);

        if std::env::var_os("CELESTINE_NO_UPDATE").is_some() || current.is_none() {
            let _ = tx.send(CheckMsg::None);
            return Self {
                rx,
                available: None,
                settled: false,
            };
        }
        let current = current.expect("checked");

        thread::Builder::new()
            .name("celestine-update-check".into())
            .spawn(move || {
                let msg = match fetch_latest() {
                    Some(info) if should_notify(current, info.version) => CheckMsg::Available(info),
                    _ => CheckMsg::None,
                };
                let _ = tx.send(msg);
            })
            .ok();

        Self {
            rx,
            available: None,
            settled: false,
        }
    }

    /// A checker that never fetches anything (tests).
    #[cfg(test)]
    pub fn idle() -> Self {
        let (_tx, rx) = mpsc::channel();
        Self {
            rx,
            available: None,
            settled: true,
        }
    }

    pub fn pending(&self) -> bool {
        !self.settled
    }

    /// Poll the worker; call once per frame until settled.
    pub fn poll(&mut self) -> bool {
        if self.settled {
            return false;
        }
        match self.rx.try_recv() {
            Ok(CheckMsg::Available(info)) => {
                self.available = Some(info);
                self.settled = true;
                true
            }
            Ok(CheckMsg::None) => {
                self.settled = true;
                false
            }
            Err(mpsc::TryRecvError::Empty) => false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.settled = true;
                false
            }
        }
    }

    pub fn available(&self) -> Option<&UpdateInfo> {
        self.available.as_ref()
    }

    pub fn clear(&mut self) {
        self.available = None;
    }
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
}

fn fetch_latest() -> Option<UpdateInfo> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(8))
        .build();

    let resp = agent
        .get(API_LATEST)
        .set(
            "User-Agent",
            concat!("CelestineMD/", env!("CARGO_PKG_VERSION")),
        )
        .set("Accept", "application/vnd.github+json")
        .call()
        .ok()?;

    if !(200..300).contains(&resp.status()) {
        return None;
    }

    let release: GhRelease = resp.into_json().ok()?;
    let version = SemVer::parse(&release.tag_name)?;
    Some(UpdateInfo {
        version,
        html_url: if release.html_url.is_empty() {
            RELEASES_PAGE.to_string()
        } else {
            release.html_url
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_semver() {
        assert_eq!(
            SemVer::parse("0.2.0"),
            Some(SemVer {
                major: 0,
                minor: 2,
                patch: 0
            })
        );
        assert_eq!(
            SemVer::parse("v0.2.1"),
            Some(SemVer {
                major: 0,
                minor: 2,
                patch: 1
            })
        );
        assert_eq!(SemVer::parse("1.0.0-rc.1"), None);
    }

    #[test]
    fn notify_feature_line_only() {
        let cur = SemVer::parse("0.2.0").unwrap();
        assert!(!should_notify(cur, SemVer::parse("0.2.1").unwrap()));
        assert!(!should_notify(cur, SemVer::parse("0.2.9").unwrap()));
        assert!(should_notify(cur, SemVer::parse("0.3.0").unwrap()));
        assert!(should_notify(cur, SemVer::parse("0.3.2").unwrap()));
        assert!(should_notify(cur, SemVer::parse("1.0.0").unwrap()));

        let on_patch = SemVer::parse("0.2.5").unwrap();
        assert!(!should_notify(on_patch, SemVer::parse("0.2.8").unwrap()));
        assert!(should_notify(on_patch, SemVer::parse("0.3.0").unwrap()));
    }
}
