//! The run and debug configuration written by csolution (`*.cbuild-run.yml`).

use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Deserialize)]
pub struct File {
    #[serde(rename = "cbuild-run")]
    pub run: CbuildRun,
}

#[derive(Debug, Default, Deserialize)]
pub struct CbuildRun {
    pub solution: Option<String>,
    #[serde(rename = "target-type")]
    pub target_type: Option<String>,
    pub board: Option<String>,
    pub device: Option<String>,
    #[serde(default)]
    pub output: Vec<Output>,
    #[serde(rename = "system-descriptions", default)]
    pub system_descriptions: Vec<Description>,
    pub debugger: Option<Debugger>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Output {
    pub file: String,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub load: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Description {
    pub file: String,
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Debugger {
    pub name: Option<String>,
    #[serde(default)]
    pub gdbserver: Vec<GdbServer>,
    pub telnet: Option<serde_yaml_ng::Value>,
}

#[derive(Debug, Default, Deserialize)]
pub struct GdbServer {
    pub port: Option<u16>,
}

/// A parsed cbuild-run file with paths resolved.
#[derive(Debug)]
pub struct RunConfig {
    pub dir: PathBuf,
    pub run: CbuildRun,
}

impl RunConfig {
    pub fn load(path: &Path) -> Result<RunConfig, crate::LoadError> {
        let text = std::fs::read_to_string(path).map_err(|e| crate::LoadError(format!("{}: {e}", path.display())))?;
        let f: File = serde_yaml_ng::from_str(&text).map_err(|e| crate::LoadError(format!("{}: {e}", path.display())))?;
        Ok(RunConfig { dir: path.parent().unwrap_or(Path::new(".")).to_path_buf(), run: f.run })
    }

    fn resolve(&self, f: &str) -> PathBuf {
        let expanded = expand_vars(f);
        let p = PathBuf::from(&expanded);
        if p.is_absolute() {
            p
        } else {
            self.dir.join(p)
        }
    }

    /// Image files to load (`load: image` or `image+symbols`; ELF preferred over HEX).
    pub fn images(&self) -> Vec<PathBuf> {
        self.run.output.iter().filter(|o| o.load.as_deref().is_some_and(|l| l.contains("image"))).map(|o| self.resolve(&o.file)).collect()
    }

    /// Symbol files (`load: symbols` or `image+symbols`).
    pub fn symbol_files(&self) -> Vec<PathBuf> {
        self.run.output.iter().filter(|o| o.load.as_deref().is_some_and(|l| l.contains("symbols"))).map(|o| self.resolve(&o.file)).collect()
    }

    pub fn svd(&self) -> Option<PathBuf> {
        self.run.system_descriptions.iter().find(|d| d.kind.as_deref() == Some("svd")).map(|d| self.resolve(&d.file))
    }

    pub fn gdb_port(&self) -> Option<u16> {
        self.run.debugger.as_ref()?.gdbserver.first()?.port
    }
}

/// Expands `${VAR}` with the environment; `${CMSIS_PACK_ROOT}` defaults to the standard location.
pub fn expand_vars(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find('}') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let name = &rest[i + 2..i + j];
        let val = std::env::var(name).ok().or_else(|| if name == "CMSIS_PACK_ROOT" { default_pack_root() } else { None });
        out.push_str(&val.unwrap_or_default());
        rest = &rest[i + j + 1..];
    }
    out.push_str(rest);
    out
}

fn default_pack_root() -> Option<String> {
    if cfg!(windows) {
        std::env::var("LOCALAPPDATA").ok().map(|d| format!("{d}\\Arm\\Packs"))
    } else {
        std::env::var("HOME").ok().map(|h| format!("{h}/.cache/arm/packs"))
    }
}
