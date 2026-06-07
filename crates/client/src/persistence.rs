use crate::activity::{ActivityState, ActivityStateConsumer};
use crate::credit::utilization::{CreditUtilizationConsumer, CreditUtilizationState};
use crate::pain::state::{PainEntry, PainState};
use crate::pain::{PainCatalog, PainConsumer, PainLabelStore};
use crate::usage::AllUsageConsumer;
use crate::usage::all::AllState;
use crate::usage::breaks::BreakState;
use crate::usage::daily::DayState;
use crate::usage::rest::RestState;
use crate::watch_mux::{WatchMux, define_watch_mux_4};
use bachelor::error::Closed;
use directories::ProjectDirs;
use jiff::Timestamp;
use litemap::LiteMap;
use rootcause::prelude::*;
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{debug, error, info};
use uuid::Uuid;

/// Minimum interval between saves after a state change is observed.
const DEBOUNCE: Duration = Duration::from_secs(60);

/// How long an unconfigured (orphaned) pain entry is kept in the
/// persisted file after its `last_updated` timestamp. Protects against
/// permanently losing state from a brief config rename or typo while
/// still letting genuinely-removed sources fall off.
const ORPHAN_RETENTION: Duration = Duration::from_secs(3600);

/// On-disk form of the persisted application state. The pain map is
/// string-keyed and merges configured + orphaned entries; conversion to
/// the in-memory [`AppSnapshot`] partitions them via the catalog and
/// prunes stale orphans.
#[derive(Deserialize)]
struct AppSnapshotFile {
    all: AllState,
    day: DayState,
    rest: RestState,
    breaks: BreakState,
    #[serde(default)]
    pain: BTreeMap<String, PainEntry>,
    utilization: CreditUtilizationState,
    activity: ActivityState,
    identity: AppStateIdentity,
}

/// In-memory form of the persisted application state, used by callers
/// after loading.
#[derive(Default)]
pub struct AppSnapshot {
    pub all: AllState,
    pub day: DayState,
    pub rest: RestState,
    pub breaks: BreakState,
    pub pain: PainState,
    pub utilization: CreditUtilizationState,
    pub activity: ActivityState,
}

/// Persisted identity of the application state. Round-tripped by the
/// persistence driver rather than living in any live watch.
///
/// - `app_state_id` is created once when persisted state is first
///   initialized and is stable across saves.
/// - `app_state_basis` increments only when the cumulative-counter basis is
///   intentionally reset/invalidated. Round-tripped unchanged today.
/// - `app_state_generation` increments on every persist and identifies which
///   persisted-state revision a reader started from.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AppStateIdentity {
    pub app_state_id: Uuid,
    pub app_state_basis: u64,
    pub app_state_generation: u64,
}

impl AppStateIdentity {
    /// Fresh identity for a state that has never been persisted: a new id,
    /// the base basis, and generation zero.
    pub fn initialize() -> Self {
        Self {
            app_state_id: Uuid::new_v4(),
            app_state_basis: 0,
            app_state_generation: 0,
        }
    }
}

impl Default for AppStateIdentity {
    fn default() -> Self {
        Self::initialize()
    }
}

/// Round-tripped data that is read from the persisted file but is not
/// part of any live application state. Owned by the persistence driver
/// for the lifetime of the process and merged back into the file on
/// every save so a brief config rename or typo doesn't permanently
/// drop information.
#[derive(Default)]
pub struct PersistBaggage {
    pub pain: PainBaggage,
}

impl PersistBaggage {
    /// Drop any round-tripped data that has aged past its retention.
    /// Called by the persistence driver before each save so a long-lived
    /// process doesn't keep writing entries that are already past expiry.
    pub fn clean(&mut self) {
        self.pain.clean();
    }
}

/// Pain entries loaded from disk whose label is not present in the
/// configured pain sources. Carried unchanged through saves until
/// either the label reappears in config (next load promotes it back
/// to live state) or [`ORPHAN_RETENTION`] elapses.
#[derive(Default)]
pub struct PainBaggage {
    pub orphans: BTreeMap<String, PainEntry>,
}

impl PainBaggage {
    fn clean(&mut self) {
        let now = Timestamp::now();
        self.orphans.retain(|name, entry| {
            let keep = now.duration_since(entry.last_updated()).unsigned_abs() <= ORPHAN_RETENTION;
            if !keep {
                debug!("Dropping stale orphan pain entry: {name}");
            }
            keep
        });
    }
}

impl AppSnapshotFile {
    /// Convert the on-disk form into the in-memory snapshot plus the
    /// per-process baggage. Pain entries whose label is in `labels`
    /// become interned `PainState.entries`; the rest go into the
    /// baggage as orphans, then [`PersistBaggage::clean`] prunes any
    /// already past [`ORPHAN_RETENTION`].
    fn split(self, labels: &PainLabelStore) -> (AppSnapshot, AppStateIdentity, PersistBaggage) {
        let mut entries = LiteMap::new();
        let mut orphans = BTreeMap::new();
        for (name, entry) in self.pain {
            match labels.get(&name) {
                Some(label) => {
                    entries.insert(label, entry);
                }
                None => {
                    orphans.insert(name, entry);
                }
            }
        }
        let snapshot = AppSnapshot {
            all: self.all,
            day: self.day,
            rest: self.rest,
            breaks: self.breaks,
            pain: PainState { entries },
            utilization: self.utilization,
            activity: self.activity,
        };
        let mut baggage = PersistBaggage {
            pain: PainBaggage { orphans },
        };
        baggage.clean();
        (snapshot, self.identity, baggage)
    }
}

/// Borrowed serialize-side wrapper over a live [`PainState`] plus the
/// driver-owned orphan map. Resolves each interned label through
/// `catalog` and merges in the orphans so the on-disk form is a single
/// combined string-keyed map.
struct PainStateRef<'a> {
    state: &'a PainState,
    catalog: &'a PainCatalog,
    orphans: &'a BTreeMap<String, PainEntry>,
}

impl Serialize for PainStateRef<'_> {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        let total = self.state.entries.len() + self.orphans.len();
        let mut map = ser.serialize_map(Some(total))?;
        for (label, entry) in &self.state.entries {
            map.serialize_entry(self.catalog.resolve(*label), entry)?;
        }
        for (name, entry) in self.orphans {
            map.serialize_entry(name, entry)?;
        }
        map.end()
    }
}

/// Borrowed form of the persisted application state, used when saving
/// so that we never have to clone the live state out of its watch.
#[derive(Serialize)]
struct AppSnapshotRef<'a> {
    all: &'a AllState,
    day: &'a DayState,
    rest: &'a RestState,
    breaks: &'a BreakState,
    pain: PainStateRef<'a>,
    utilization: &'a CreditUtilizationState,
    activity: &'a ActivityState,
    identity: &'a AppStateIdentity,
}

#[derive(Deserialize)]
#[serde(tag = "schema_version", content = "data")]
enum AppStateFileSchema {
    #[serde(rename = "1")]
    V1(AppSnapshotFile),
}

#[derive(Serialize)]
#[serde(tag = "schema_version", content = "data")]
enum AppStateFileSchemaRef<'a> {
    #[serde(rename = "1")]
    V1(AppSnapshotRef<'a>),
}

#[derive(Deserialize)]
struct AppStateFile {
    app_version: String,
    #[serde(flatten)]
    schema: AppStateFileSchema,
}

#[derive(Serialize)]
struct AppStateFileRef<'a> {
    app_version: &'a str,
    #[serde(flatten)]
    schema: AppStateFileSchemaRef<'a>,
}

/// Default path for the persisted state file.
///
/// Prefers `$XDG_STATE_HOME/openergo/state.json` on Linux, falling
/// back to the data directory if no state directory is available.
pub fn default_state_path() -> PathBuf {
    let dir = ProjectDirs::from("", "", "openergo")
        .map(|dirs| {
            dirs.state_dir()
                .unwrap_or_else(|| dirs.data_dir())
                .to_path_buf()
        })
        .unwrap_or_else(|| PathBuf::from("."));
    dir.join("state.json")
}

fn suffixed(base: &Path, suffix: &str) -> PathBuf {
    let mut p = base.as_os_str().to_owned();
    p.push(suffix);
    PathBuf::from(p)
}

fn next_path(current: &Path) -> PathBuf {
    suffixed(current, ".next")
}

fn prev_path(current: &Path) -> PathBuf {
    suffixed(current, ".prev")
}

fn tmp_path(next: &Path) -> PathBuf {
    suffixed(next, ".tmp")
}

/// Load the persisted snapshot, trying the freshness-ordered slots in
/// turn: `.next` (a write that crashed before promotion), the current
/// file, and finally the `.prev` backup. Returns the in-memory snapshot
/// (if any slot was loadable), the persisted [`AppStateIdentity`] (freshly
/// initialized when no slot exists), plus the [`PersistBaggage`] that the
/// driver must hold and round-trip on every save. Missing files are
/// logged at debug level and skipped.
///
/// An existing slot that fails to read or parse is fatal: returning
/// `Ok(None)` would let the app start on older state and the
/// persistence driver would then overwrite every slot, destroying the
/// user's data. The caller must surface the error and refuse to start.
///
/// Pain entries whose label is in `labels` are returned interned in
/// the snapshot; entries for unconfigured labels are kept in the
/// baggage (subject to [`ORPHAN_RETENTION`]) so a brief rename or
/// config typo does not permanently wipe them.
pub async fn load(
    path: &Path,
    labels: &PainLabelStore,
) -> Result<(Option<AppSnapshot>, AppStateIdentity, PersistBaggage), Report> {
    let next = next_path(path);
    let prev = prev_path(path);

    for candidate in [next.as_path(), path, prev.as_path()] {
        if !tokio::fs::try_exists(candidate).await.unwrap_or(false) {
            debug!("State slot {} not present", candidate.display());
            continue;
        }
        let file = try_load_from_file(candidate)
            .await
            .context("Failed to load app state")
            .attach(format!("path: {}", candidate.display()))?;
        info!("Loaded app state from {}", candidate.display());
        check_version_and_backup(candidate, &file.app_version).await?;
        let AppStateFileSchema::V1(inner) = file.schema;
        let (snapshot, identity, baggage) = inner.split(labels);
        return Ok((Some(snapshot), identity, baggage));
    }
    Ok((
        None,
        AppStateIdentity::initialize(),
        PersistBaggage::default(),
    ))
}

async fn try_load_from_file(path: &Path) -> Result<AppStateFile, Report> {
    let content = tokio::fs::read_to_string(path)
        .await
        .context("Failed to read state file")
        .attach(format!("path: {}", path.display()))?;
    let file: AppStateFile =
        serde_json::from_str(&content).context("Failed to parse state file")?;
    Ok(file)
}

/// Compare the file's recorded `app_version` against the currently
/// running build. Refuse to load anything written by a newer app
/// (forward-incompatible by policy). When the version differs but is
/// older, back the source file up once to `<path>.<old_version>.bak`
/// so we keep an untouched copy from before this build started writing
/// over it. Idempotent: skips if the backup already exists.
async fn check_version_and_backup(path: &Path, file_version: &str) -> Result<(), Report> {
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .context("Failed to parse current app version")?;
    let file = semver::Version::parse(file_version)
        .context("Failed to parse app_version in state file")
        .attach(format!("app_version: {file_version}"))?;

    if file > current {
        return Err(
            report!("State file was written by a newer app version; refusing to load")
                .attach(format!("file app_version: {file}"))
                .attach(format!("current app_version: {current}")),
        );
    }

    if file == current {
        return Ok(());
    }

    let backup = suffixed(path, &format!(".{file}.bak"));
    if tokio::fs::try_exists(&backup).await.unwrap_or(false) {
        debug!(
            "Backup {} already exists; not overwriting",
            backup.display()
        );
        return Ok(());
    }
    tokio::fs::copy(path, &backup)
        .await
        .context("Failed to back up state file before version change")
        .attach(format!("src: {}", path.display()))
        .attach(format!("dst: {}", backup.display()))?;
    info!(
        "App version changed ({} -> {}); backed up prior state to {}",
        file,
        current,
        backup.display()
    );
    Ok(())
}

/// Serialize the snapshot reference into a JSON string. Pure CPU work,
/// suitable for running inside the nested `view` closures so we never
/// have to materialize an owned `AppSnapshot`.
fn render(snapshot: AppSnapshotRef<'_>) -> Result<String, Report> {
    let file = AppStateFileRef {
        app_version: env!("CARGO_PKG_VERSION"),
        schema: AppStateFileSchemaRef::V1(snapshot),
    };
    let json = serde_json::to_string(&file).context("Failed to serialize state")?;
    Ok(json)
}

/// Save a pre-rendered state file payload using a three-slot atomic
/// protocol: `.next` (newest, written first), the current file, and
/// `.prev` (previous good copy). At every crash point the freshest
/// successfully-written content is recoverable by `load`.
pub async fn save(path: &Path, content: &str) -> Result<(), Report> {
    let next = next_path(path);
    let prev = prev_path(path);

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent)
            .await
            .context("Failed to create state directory")
            .attach(format!("path: {}", parent.display()))?;
    }

    atomic_write_with_backup(path, &prev, &next, content)
        .await
        .context("Failed to write state file")?;
    Ok(())
}

/// Three-slot save protocol:
///
/// 1. Write `content` to `.next.tmp`, fsync the file, rename to `.next`.
/// 2. If `current` exists, rename it to `.prev` (atomically replacing
///    any stale `.prev`).
/// 3. Rename `.next` to `current`.
///
/// Each rename is followed by a directory fsync so the rename itself
/// is durable across power loss. `load` reads in freshness order
/// (`.next` -> current -> `.prev`), so an interruption at any step
/// leaves the freshest committed content visible to the next load.
async fn atomic_write_with_backup(
    current: &Path,
    prev: &Path,
    next: &Path,
    content: &str,
) -> Result<(), std::io::Error> {
    use tokio::io::AsyncWriteExt;

    let parent = current
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let tmp = tmp_path(next);

    // Open the parent directory once and reuse the fd for all the
    // post-rename fsyncs in this save.
    let parent_fd = tokio::fs::File::open(parent).await?;

    // 1. Write content to a temp file, fsync, then promote to `.next`.
    {
        let mut f = tokio::fs::File::create(&tmp).await?;
        f.write_all(content.as_bytes()).await?;
        f.sync_all().await?;
    }
    tokio::fs::rename(&tmp, next).await?;
    parent_fd.sync_all().await?;

    // 2. Rotate the old current into `.prev`, replacing any stale
    //    `.prev` from a previous interrupted save.
    if tokio::fs::try_exists(current).await.unwrap_or(false) {
        tokio::fs::rename(current, prev).await?;
        parent_fd.sync_all().await?;
    }

    // 3. Promote `.next` to current.
    tokio::fs::rename(next, current).await?;
    parent_fd.sync_all().await?;

    Ok(())
}

/// Spawns the persistence driver future. The driver watches all live state
/// sources and persists at most once per debounce interval while changes keep
/// arriving. The `baggage` returned from [`load`] is owned by the driver and
/// merged back into the file on every save. When every input has closed, it
/// persists one final snapshot and exits.
#[allow(clippy::too_many_arguments)]
pub fn create(
    path: PathBuf,
    sources: AllUsageConsumer,
    pain: PainConsumer,
    utilization: CreditUtilizationConsumer,
    activity: ActivityStateConsumer,
    identity: AppStateIdentity,
    baggage: PersistBaggage,
) -> impl Future<Output = Result<(), Report>> {
    let driver = Driver {
        path,
        inputs: WatchMux::new(PersistenceInputs {
            usage: sources,
            pain,
            utilization,
            activity,
        }),
        identity,
        baggage,
    };
    driver.run()
}

struct Driver {
    path: PathBuf,
    inputs: WatchMux<PersistenceInputs>,
    identity: AppStateIdentity,
    baggage: PersistBaggage,
}

define_watch_mux_4! {
    struct PersistenceInputs;
    flags PersistenceInput;
    usage: AllUsageConsumer => USAGE,
    pain: PainConsumer => PAIN,
    utilization: CreditUtilizationConsumer => UTILIZATION,
    activity: ActivityStateConsumer => ACTIVITY,
}

impl Driver {
    async fn run(mut self) -> Result<(), Report> {
        let Self {
            path,
            inputs,
            identity,
            baggage,
        } = &mut self;

        loop {
            let exit = match inputs.changed().await {
                Ok(_) => tokio::time::timeout(DEBOUNCE, inputs.closed())
                    .await
                    .is_ok(),
                Err(Closed) => true,
            };

            // Render the snapshot via nested views so we serialize
            // directly from borrowed state without cloning either the
            // usage states or the pain map. The orphan map lives on the
            // driver's baggage and is merged in by `PainStateRef`.
            baggage.clean();
            // Each persist is a new state generation; the id and basis are
            // round-tripped unchanged.
            identity.app_state_generation += 1;
            let inputs = inputs.get();
            let render_result = inputs.pain.view(|pain, catalog| {
                inputs.usage.view(|all, rest, breaks, day| {
                    inputs.utilization.view(|utilization| {
                        inputs.activity.view(|activity| {
                            render(AppSnapshotRef {
                                all,
                                day,
                                rest,
                                breaks,
                                pain: PainStateRef {
                                    state: pain,
                                    catalog,
                                    orphans: &baggage.pain.orphans,
                                },
                                utilization,
                                activity,
                                identity,
                            })
                        })
                    })
                })
            });

            match render_result {
                Ok(json) => {
                    if let Err(e) = save(path, &json).await {
                        error!("Failed to save app state: {e}");
                    } else {
                        debug!("Saved app state to {}", path.display());
                    }
                }
                Err(e) => error!("Failed to render app state: {e}"),
            }

            if exit {
                debug!("persistence driver exiting");
                return Ok(());
            }
        }
    }
}
