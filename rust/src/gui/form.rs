//! The job a window edits: a `RunConfiguration` plus the directory its result goes to and the text of the JSON tab.
//! There are no window types here, so loading, saving and the JSON round trip are tested without opening one.

use crate::config::run_configuration::{BlocksConfig, RunConfiguration};
use crate::data::loader::read_resource_or_file;
use anyhow::{Context, Result, bail};

/// The schema example compiled in from `src/main/resources/example_config.json`: every field in one place, for the
/// JSON tab. It does not plan (its `forbidden_places` and fixed source tiles leave the LP infeasible), so it is not
/// what the window opens with.
pub const EXAMPLE: &str = "example_config.json";

/// The six-science job the command line runs with no `--config`: 618 machines, too big to watch on a first run.
pub const CONFIG: &str = "config.json";

/// The job the window opens with when the repository is next to it: ten machines, so the first run takes seconds.
pub const DEFAULT: &str = "../docs/examples/red_green_science.json";

/// `BlocksConfig::max_machines` has no usable default (0 fails validation), so switching the section on picks one.
const BLOCKS_MACHINES: i32 = 60;

/// What the window edits.
pub struct JobForm {
    pub config: RunConfiguration,
    /// Where the run writes. Empty means `solves/<stamp> - gui`, the way the command line names its runs.
    pub out: String,
    /// Where Load and Save read and write.
    pub path: String,
    /// The JSON tab's text.
    pub json: String,
    pub json_error: Option<String>,
    /// True while `json` holds edits the form has not taken, so `sync_json` leaves them alone.
    pub json_edited: bool,
}

impl JobForm {
    pub fn new(config: RunConfiguration) -> Self {
        Self {
            json: to_json(&config),
            config,
            out: String::new(),
            path: String::new(),
            json_error: None,
            json_edited: false,
        }
    }

    /// The schema example: every field in one place. It loads and validates but does not plan, so it is there to be
    /// looked at in the JSON tab, not run.
    pub fn example() -> Result<Self> {
        Self::load(EXAMPLE)
    }

    /// The job the window opens with: the small red-and-green job in the repository when it is there, else the job
    /// the command line defaults to.
    pub fn default_job() -> Result<Self> {
        Self::load(DEFAULT).or_else(|_| Self::load(CONFIG))
    }

    /// Jobs the window offers without typing a path: the ones in `docs/`, then the command line's own default. The
    /// schema example is deliberately not among them.
    pub fn presets() -> Vec<String> {
        let mut presets = Vec::new();
        for dir in ["../docs/examples", "../docs/status"] {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            let mut found: Vec<String> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
                .map(|path| path.to_string_lossy().into_owned())
                .collect();
            found.sort();
            presets.extend(found);
        }
        presets.push(CONFIG.to_owned());
        presets
    }

    /// A job from a built-in resource name or a path. Validates, because `RunConfiguration::parse` does: text that
    /// would not run is refused with its reason rather than opened into a form that cannot start.
    pub fn load(path: &str) -> Result<Self> {
        let text = read_resource_or_file(path)?;
        let config = RunConfiguration::parse(&text).with_context(|| format!("Bad job {path}"))?;
        let mut form = Self::new(config);
        form.path = path.to_owned();
        Ok(form)
    }

    /// Writes the job as it stands, drafts included: the window already says what is wrong with it, and a half-built
    /// job is worth keeping.
    pub fn save(&self) -> Result<()> {
        if self.path.trim().is_empty() {
            bail!("no path to save to");
        }
        std::fs::write(&self.path, to_json(&self.config)).with_context(|| format!("Could not write {}", self.path))
    }

    pub fn validate(&self) -> Result<()> {
        self.config.validate()
    }

    /// Regenerates the JSON tab from the form, unless the tab holds edits of its own.
    pub fn sync_json(&mut self) {
        if !self.json_edited {
            self.json = to_json(&self.config);
        }
    }

    /// Takes the JSON tab into the form, leaving the form alone when the text does not parse or does not validate.
    pub fn apply_json(&mut self) -> Result<()> {
        match RunConfiguration::parse(&self.json) {
            Ok(config) => {
                self.config = config;
                self.json_edited = false;
                self.json_error = None;
                Ok(())
            }
            Err(e) => {
                self.json_error = Some(format!("{e:#}"));
                Err(e)
            }
        }
    }

    /// Switches the blocks section on (sized so it validates) or off; leaves it alone when it is already that way, so
    /// toggling twice does not throw the settings away.
    pub fn toggle_blocks(&mut self, on: bool) {
        match (on, self.config.blocks.is_some()) {
            (true, false) => self.config.blocks = Some(BlocksConfig { max_machines: BLOCKS_MACHINES, ..BlocksConfig::default() }),
            (false, true) => self.config.blocks = None,
            _ => {}
        }
    }

    /// Turns a trick on or off by writing it into the job, so the file records the choice even where it matches the
    /// default.
    pub fn toggle_trick(&mut self, id: &str, on: bool) {
        self.config.tricks.insert(id.to_owned(), on);
    }

    /// Where this job's output goes: its own directory when one is set, else a stamped one like the command line's.
    pub fn out_dir(&self, stamp: &str) -> String {
        match self.out.trim() {
            "" => format!("solves/{stamp} - gui"),
            dir => dir.to_owned(),
        }
    }
}

fn to_json(config: &RunConfiguration) -> String {
    serde_json::to_string_pretty(config).expect("a job configuration serialises")
}
