pub mod exclusions;
pub mod notes;
pub mod settings;
pub mod social;

pub use exclusions::{ExclusionList, HotkeyChord, HotkeyText, SessionExclusions};
pub use notes::{DossierNote, DossierNotes};
pub use settings::{
    AppSettings, AppSettingsLoadResult, AppSettingsToml, CaptureMethod, DarkMode, ImportMode,
    OrgDisplay,
};
pub use social::{
    AttributeKind, AttributeTarget, ColorRanks, ColorResolver, ConfigLoadResult,
    HostedConfigClient, HostedConfigUrl, OrgAttributeRule, OrgEntry, OrgVocabulary, PlayerEntry,
    SocialConfig, SocialConfigMerger, SocialConfigToml,
};

use directories::ProjectDirs;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Resolves `Config/`, `Data/`, and `Logs/` directories cleanly on both Windows 11 and Bazzite Linux.
///
/// Mirrors IRIS's portable layout (`<exe_dir>/Config`, `<exe_dir>/Data`, `<exe_dir>/Logs`) when
/// `force_portable` is true or `<exe_dir>/Config` already exists and is writable, and otherwise
/// uses OS-appropriate XDG / AppData directories (`~/.config/sc-overlay`, `~/.local/share/sc-overlay`
/// on Linux/Bazzite; `%LOCALAPPDATA%\sc-overlay` on Windows).
#[derive(Debug, Clone)]
pub struct DataPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub logs_dir: PathBuf,
}

impl DataPaths {
    pub fn resolve(force_portable: bool) -> Self {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                let portable_cfg = exe_dir.join("Config");
                let use_portable = force_portable
                    || (portable_cfg.is_dir() && is_dir_writable(&portable_cfg))
                    || (cfg!(windows) && !is_system_install_dir(exe_dir) && is_dir_writable(exe_dir));

                if use_portable {
                    let paths = Self {
                        config_dir: portable_cfg,
                        data_dir: exe_dir.join("Data"),
                        logs_dir: exe_dir.join("Logs"),
                    };
                    let _ = paths.ensure_dirs();
                    return paths;
                }
            }
        }

        if let Some(proj) = ProjectDirs::from("io", "sc-overlay", "sc-overlay") {
            let paths = Self {
                config_dir: proj.config_dir().to_path_buf(),
                data_dir: proj.data_local_dir().to_path_buf(),
                logs_dir: proj.data_local_dir().join("Logs"),
            };
            let _ = paths.ensure_dirs();
            return paths;
        }

        let fallback = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let paths = Self {
            config_dir: fallback.join("Config"),
            data_dir: fallback.join("Data"),
            logs_dir: fallback.join("Logs"),
        };
        let _ = paths.ensure_dirs();
        paths
    }

    pub fn for_root(root: impl AsRef<Path>) -> Self {
        let r = root.as_ref();
        Self {
            config_dir: r.join("Config"),
            data_dir: r.join("Data"),
            logs_dir: r.join("Logs"),
        }
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        fs::create_dir_all(&self.config_dir)?;
        fs::create_dir_all(&self.data_dir)?;
        fs::create_dir_all(&self.logs_dir)?;
        Ok(())
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("settings.txt")
    }

    pub fn social_config_file(&self) -> PathBuf {
        self.config_dir.join("social-config.txt")
    }

    pub fn dossier_notes_file(&self) -> PathBuf {
        self.config_dir.join("dossier-notes.txt")
    }

    pub fn mining_file(&self) -> PathBuf {
        self.config_dir.join("mining.txt")
    }

    pub fn exclusions_file(&self) -> PathBuf {
        self.config_dir.join("exclusions.txt")
    }

    pub fn cache_db_file(&self) -> PathBuf {
        self.data_dir.join("cache.db")
    }

    pub fn breadcrumbs_file(&self) -> PathBuf {
        self.data_dir.join("breadcrumbs.json")
    }

    pub fn seed_defaults_if_missing(&self) {
        let _ = self.ensure_dirs();
        let seed = |path: PathBuf, content: &str| {
            if !path.exists() {
                let _ = AtomicFile::write_text(&path, content, false);
            }
        };
        seed(
            self.settings_file(),
            include_str!("../../../../assets/settings.txt"),
        );
        seed(
            self.social_config_file(),
            include_str!("../../../../assets/social-config.txt"),
        );
        seed(
            self.mining_file(),
            include_str!("../../../../assets/mining.txt"),
        );
        seed(
            self.exclusions_file(),
            include_str!("../../../../assets/exclusions.txt"),
        );
    }
}

fn is_system_install_dir(dir: &Path) -> bool {
    let s = dir.to_string_lossy().to_ascii_lowercase();
    s.contains("program files")
        || s.starts_with("/usr")
        || s.starts_with("/opt")
        || s.starts_with("/bin")
        || s.contains("/target/debug")
        || s.contains("\\target\\debug")
        || s.contains("/target/release")
        || s.contains("\\target\\release")
}

fn is_dir_writable(dir: &Path) -> bool {
    if !dir.exists() {
        return false;
    }
    let probe = dir.join(format!(".sc_overlay_probe_{}", std::process::id()));
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(f) => {
            drop(f);
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

pub struct AtomicFile;

impl AtomicFile {
    pub fn write_text(path: &Path, text: &str, keep_backup: bool) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("config.txt");
        let tmp_path = path.with_file_name(format!(".{}.{}.tmp", file_name, std::process::id()));
        {
            let mut f = fs::File::create(&tmp_path)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
        }
        if keep_backup && path.exists() {
            let bak_path = path.with_file_name(format!("{}.bak", file_name));
            let _ = fs::copy(path, &bak_path);
        }
        match fs::rename(&tmp_path, path) {
            Ok(()) => Ok(()),
            Err(_) => {
                // On Windows, fs::rename may fail if destination is locked or read-only attribute differs
                fs::copy(&tmp_path, path)?;
                let _ = fs::remove_file(&tmp_path);
                Ok(())
            }
        }
    }
}

