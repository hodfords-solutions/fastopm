//! Memory budgets: the `fastopm.memory_limit`, `manifest_cache`, `render_cache` and
//! `work_memory` options, and the defaults derived from the container memory limit.
//!
//! fastopm keeps three bounded pools of memory:
//!
//! - `manifest_cache`: parsed manifests (`serde_json` trees, counted at their real heap size);
//! - `render_cache`: rendered response bodies and their compressed copies;
//! - `work_memory`: transient memory of work in progress — uplink bodies being read and parsed,
//!   merges and disk writes, renders and compression of bodies not served from the render
//!   cache, tarballs being downloaded. Work that does not fit waits (requests are delayed, not
//!   failed); one item larger than the whole budget runs alone.
//!
//! Each option takes a size; when absent it is a share of `memory_limit` (see
//! [`MemoryPlan::resolve`]). `memory_limit` itself defaults to the cgroup memory limit of the
//! container (cgroup v2 `memory.max`, else v1 `memory.limit_in_bytes`), else the machine's
//! `MemTotal`, else 2 GiB.

use std::path::Path;

/// 1 MiB.
const MIB: u64 = 1024 * 1024;
/// 1 GiB.
const GIB: u64 = 1024 * MIB;

/// `memory_limit` when nothing is configured or detected.
pub const FALLBACK_MEMORY_LIMIT: u64 = 2 * GIB;
/// Default `manifest_cache`: this share of `memory_limit` (percent), at most [`MANIFEST_CACHE_MAX`].
pub const MANIFEST_CACHE_PERCENT: u64 = 6;
/// Upper bound of the default `manifest_cache`.
pub const MANIFEST_CACHE_MAX: u64 = 256 * MIB;
/// Default `render_cache`: this share of `memory_limit` (percent), at most [`RENDER_CACHE_MAX`].
pub const RENDER_CACHE_PERCENT: u64 = 10;
/// Upper bound of the default `render_cache`.
pub const RENDER_CACHE_MAX: u64 = 512 * MIB;
/// Default `work_memory`: this share of `memory_limit` (percent), at most [`WORK_MEMORY_MAX`].
pub const WORK_MEMORY_PERCENT: u64 = 12;
/// Upper bound of the default `work_memory`.
pub const WORK_MEMORY_MAX: u64 = GIB;

/// Where `memory_limit` came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitSource {
    /// `fastopm.memory_limit` in the config file.
    Config,
    /// The cgroup memory limit (container limit).
    Cgroup,
    /// `MemTotal` of `/proc/meminfo` (no cgroup limit).
    MemTotal,
    /// Nothing configured or detected: [`FALLBACK_MEMORY_LIMIT`].
    Fallback,
}

impl std::fmt::Display for LimitSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LimitSource::Config => "fastopm.memory_limit",
            LimitSource::Cgroup => "cgroup limit",
            LimitSource::MemTotal => "MemTotal",
            LimitSource::Fallback => "default",
        })
    }
}

/// The memory options of the `fastopm:` section as written (`None` = not set).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoryOptions {
    /// `memory_limit`: memory available to fastopm, the base of the defaults below.
    pub memory_limit: Option<u64>,
    /// `manifest_cache`: bytes of parsed manifests kept in RAM; `0` disables the cache.
    pub manifest_cache: Option<u64>,
    /// `render_cache`: bytes of rendered bodies kept in RAM; `0` disables the cache.
    pub render_cache: Option<u64>,
    /// `work_memory`: bytes of transient work in flight; `0` removes the limit.
    pub work_memory: Option<u64>,
}

/// Resolved budgets, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryPlan {
    /// `memory_limit`.
    pub memory_limit: u64,
    /// Where `memory_limit` came from.
    pub source: LimitSource,
    /// Manifest cache budget (0: disabled).
    pub manifest_cache: u64,
    /// Render cache budget (0: disabled).
    pub render_cache: u64,
    /// Work budget (0: unlimited).
    pub work_memory: u64,
}

fn share(limit: u64, percent: u64, max: u64) -> u64 {
    (limit / 100 * percent).min(max)
}

impl MemoryPlan {
    /// Budgets from the options and the detected limit ([`detect_memory_limit`]):
    ///
    /// - `memory_limit` = configured, else `detected`, else [`FALLBACK_MEMORY_LIMIT`];
    /// - `manifest_cache` = configured, else 10% of `memory_limit`, at most 256 MiB;
    /// - `render_cache` = configured, else 15% of `memory_limit`, at most 512 MiB;
    /// - `work_memory` = configured, else 25% of `memory_limit`, at most 1 GiB.
    ///
    /// The other half of `memory_limit` is headroom: the process itself, allocator overhead,
    /// response bodies on their way to clients, and the one work item allowed past
    /// `work_memory`.
    pub fn resolve(options: &MemoryOptions, detected: Option<(u64, LimitSource)>) -> Self {
        let (memory_limit, source) = match (options.memory_limit, detected) {
            (Some(limit), _) => (limit, LimitSource::Config),
            (None, Some(found)) => found,
            (None, None) => (FALLBACK_MEMORY_LIMIT, LimitSource::Fallback),
        };
        Self {
            memory_limit,
            source,
            manifest_cache: options
                .manifest_cache
                .unwrap_or_else(|| share(memory_limit, MANIFEST_CACHE_PERCENT, MANIFEST_CACHE_MAX)),
            render_cache: options
                .render_cache
                .unwrap_or_else(|| share(memory_limit, RENDER_CACHE_PERCENT, RENDER_CACHE_MAX)),
            work_memory: options
                .work_memory
                .unwrap_or_else(|| share(memory_limit, WORK_MEMORY_PERCENT, WORK_MEMORY_MAX)),
        }
    }
}

/// The memory limit of this process: the cgroup limit when there is one, else `MemTotal`.
/// `None` when neither can be read (not Linux).
pub fn detect_memory_limit() -> Option<(u64, LimitSource)> {
    detect_in(Path::new("/"))
}

/// [`detect_memory_limit`] with `/sys` and `/proc` under `root` (tests).
pub fn detect_in(root: &Path) -> Option<(u64, LimitSource)> {
    let read = |p: &str| std::fs::read_to_string(root.join(p)).ok();
    // cgroup v2: "max" or a number; v1: a huge number when unlimited
    let limit = |p: &str| {
        read(p).and_then(|s| s.trim().parse::<u64>().ok()).filter(|&n| n > 0 && n < 1 << 60)
    };
    let cgroup = limit("sys/fs/cgroup/memory.max")
        .or_else(|| limit("sys/fs/cgroup/memory/memory.limit_in_bytes"));
    if let Some(limit) = cgroup {
        return Some((limit, LimitSource::Cgroup));
    }
    let meminfo = read("proc/meminfo")?;
    let kib = meminfo.lines().find_map(|l| {
        let rest = l.strip_prefix("MemTotal:")?.trim();
        rest.strip_suffix("kB").unwrap_or(rest).trim().parse::<u64>().ok()
    })?;
    Some((kib * 1024, LimitSource::MemTotal))
}

/// Parse a size: a number of bytes, or a number (fraction allowed) followed by a unit `b`,
/// `kb`, `mb`, `gb`, `tb` (powers of 1024, as Verdaccio's `max_body_size`) or `k`, `m`, `g`,
/// `t`, `ki`, `mi`, `gi`, `ti` (Kubernetes style), case-insensitive, spaces allowed between.
pub fn parse_size(text: &str) -> Option<u64> {
    let t = text.trim().to_ascii_lowercase();
    let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(t.len());
    let (number, unit) = t.split_at(split);
    let value: f64 = number.parse().ok().filter(|v: &f64| v.is_finite() && *v >= 0.0)?;
    let scale: u64 = match unit.trim() {
        "" | "b" => 1,
        "k" | "kb" | "ki" | "kib" => 1024,
        "m" | "mb" | "mi" | "mib" => MIB,
        "g" | "gb" | "gi" | "gib" => GIB,
        "t" | "tb" | "ti" | "tib" => 1024 * GIB,
        _ => return None,
    };
    let bytes = (value * scale as f64).floor();
    (bytes < u64::MAX as f64).then_some(bytes as u64)
}

/// `n` bytes as `512 MiB`, `1.5 GiB`... (for logs).
pub fn format_size(n: u64) -> String {
    if n >= GIB {
        format!("{:.1} GiB", n as f64 / GIB as f64)
    } else if n >= MIB {
        format!("{} MiB", n / MIB)
    } else {
        format!("{} KiB", n / 1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_size("1024"), Some(1024));
        assert_eq!(parse_size("10mb"), Some(10 * MIB));
        assert_eq!(parse_size("10 MB"), Some(10 * MIB));
        assert_eq!(parse_size("1.5gb"), Some(3 * GIB / 2));
        assert_eq!(parse_size("512Mi"), Some(512 * MIB));
        assert_eq!(parse_size("2Gi"), Some(2 * GIB));
        assert_eq!(parse_size("64k"), Some(64 * 1024));
        assert_eq!(parse_size("0"), Some(0));
        assert_eq!(parse_size("12b"), Some(12));
        for bad in ["", "mb", "-1mb", "10 parsecs", "1e3", "1..2mb"] {
            assert_eq!(parse_size(bad), None, "{bad}");
        }
        assert_eq!(format_size(512 * MIB), "512 MiB");
        assert_eq!(format_size(3 * GIB / 2), "1.5 GiB");
    }

    #[test]
    fn defaults_are_shares_of_the_limit_with_caps() {
        let none = MemoryOptions::default();
        let p = MemoryPlan::resolve(&none, Some((512 * MIB, LimitSource::Cgroup)));
        assert_eq!(p.source, LimitSource::Cgroup);
        assert_eq!(p.manifest_cache, 512 * MIB / 100 * 6);
        assert_eq!(p.render_cache, 512 * MIB / 100 * 10);
        assert_eq!(p.work_memory, 512 * MIB / 100 * 12);

        let p = MemoryPlan::resolve(&none, Some((64 * GIB, LimitSource::MemTotal)));
        assert_eq!((p.manifest_cache, p.render_cache, p.work_memory), (256 * MIB, 512 * MIB, GIB));

        let p = MemoryPlan::resolve(&none, None);
        assert_eq!((p.memory_limit, p.source), (FALLBACK_MEMORY_LIMIT, LimitSource::Fallback));

        let set = MemoryOptions {
            memory_limit: Some(GIB),
            manifest_cache: Some(0),
            render_cache: None,
            work_memory: Some(7),
        };
        let p = MemoryPlan::resolve(&set, Some((512 * MIB, LimitSource::Cgroup)));
        assert_eq!(p.memory_limit, GIB);
        assert_eq!(p.source, LimitSource::Config);
        assert_eq!((p.manifest_cache, p.render_cache, p.work_memory), (0, GIB / 100 * 10, 7));
    }

    #[test]
    fn detects_cgroup_v2_v1_and_meminfo() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(detect_in(root), None);
        std::fs::create_dir_all(root.join("proc")).unwrap();
        std::fs::write(root.join("proc/meminfo"), "MemTotal:        8000000 kB\nMemFree: 1 kB\n")
            .unwrap();
        assert_eq!(detect_in(root), Some((8_000_000 * 1024, LimitSource::MemTotal)));
        // v1 without a limit: ignored
        std::fs::create_dir_all(root.join("sys/fs/cgroup/memory")).unwrap();
        std::fs::write(
            root.join("sys/fs/cgroup/memory/memory.limit_in_bytes"),
            "9223372036854771712\n",
        )
        .unwrap();
        assert_eq!(detect_in(root).map(|d| d.1), Some(LimitSource::MemTotal));
        std::fs::write(root.join("sys/fs/cgroup/memory/memory.limit_in_bytes"), "536870912\n")
            .unwrap();
        assert_eq!(detect_in(root), Some((512 * MIB, LimitSource::Cgroup)));
        // v2 wins; "max" means no limit, then v1 is tried
        std::fs::write(root.join("sys/fs/cgroup/memory.max"), "max\n").unwrap();
        assert_eq!(detect_in(root), Some((512 * MIB, LimitSource::Cgroup)));
        std::fs::write(root.join("sys/fs/cgroup/memory.max"), "2147483648\n").unwrap();
        assert_eq!(detect_in(root), Some((2 * GIB, LimitSource::Cgroup)));
    }
}
