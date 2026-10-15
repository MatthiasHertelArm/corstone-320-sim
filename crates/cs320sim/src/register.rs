//! `cs320sim register-cmsis-adapter`: adds the `corstone-320-sim` debug adapter to the CMSIS
//! Solution extension and CMSIS-Toolbox catalogs, so a csolution can select it with
//! `debugger: name: corstone-320-sim`.
//!
//! Dry run by default. `--apply --backup-dir DIR` writes, after backing up every changed file
//! byte for byte (with a manifest). Existing bytes of the catalogs are kept; the entry is
//! appended. An installation that already has the entry is left unchanged.

use std::path::{Path, PathBuf};

const TEMPLATE: &str = include_str!("../../../cmsis/corstone-320-sim.adapter.json");
const ENTRY: &str = include_str!("../../../cmsis/corstone-320-sim.debug-adapter.yml");
const NAME: &str = "corstone-320-sim";
const TEMPLATE_FILE: &str = "corstone-320-sim.adapter.json";

fn newest_extension() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let mut best: Option<PathBuf> = None;
    for root in [".vscode/extensions", ".vscode-insiders/extensions", ".cursor/extensions"] {
        let dir = Path::new(&home).join(root);
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with("arm.cmsis-csolution-") && best.as_ref().is_none_or(|b| e.path() > *b) {
                best = Some(e.path());
            }
        }
    }
    best
}

/// The list items of the catalog entry (without the `debug-adapters:` header and comments).
fn entry_items() -> String {
    let mut out = String::new();
    let mut started = false;
    for l in ENTRY.lines() {
        if l.starts_with("debug-adapters:") {
            started = true;
            continue;
        }
        if started {
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

fn json_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

struct Action {
    path: PathBuf,
    new_content: Vec<u8>,
    exists: bool,
}

pub fn run(args: &[String]) -> Result<(), String> {
    let mut extension: Option<PathBuf> = None;
    let mut toolboxes: Vec<PathBuf> = Vec::new();
    let mut apply = false;
    let mut backup: Option<PathBuf> = None;
    let mut exe: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--extension" => {
                i += 1;
                extension = args.get(i).map(PathBuf::from);
            }
            "--toolbox" => {
                i += 1;
                toolboxes.extend(args.get(i).map(PathBuf::from));
            }
            "--apply" => apply = true,
            "--backup-dir" => {
                i += 1;
                backup = args.get(i).map(PathBuf::from);
            }
            "--cs320sim" => {
                i += 1;
                exe = args.get(i).cloned();
            }
            a => return Err(format!("register-cmsis-adapter: unknown option {a}")),
        }
        i += 1;
    }
    let ext = extension.or_else(newest_extension).ok_or("no CMSIS Solution extension found; pass --extension DIR")?;
    let exe = exe.unwrap_or_else(|| std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "cs320sim".into()));
    let template = TEMPLATE
        .replace("\"command\": \"cs320sim\"", &format!("\"command\": \"{}\"", json_string(&exe)))
        .replace("\"server\": \"cs320sim\"", &format!("\"server\": \"{}\"", json_string(&exe)));

    let mut catalogs = vec![ext.join("templates/debug/debug-adapters.yml"), ext.join("tools/cmsis-toolbox/etc/debug-adapters.yml")];
    catalogs.extend(toolboxes.iter().map(|t| t.join("etc/debug-adapters.yml")));

    let mut actions = Vec::new();
    for c in &catalogs {
        let Ok(text) = std::fs::read_to_string(c) else {
            println!("skip (not found): {}", c.display());
            continue;
        };
        if !text.trim_start().starts_with("debug-adapters:") && !text.contains("\ndebug-adapters:") {
            return Err(format!("{}: not a debug-adapters catalog", c.display()));
        }
        if text.contains(&format!("name: \"{NAME}\"")) {
            println!("already registered: {}", c.display());
            continue;
        }
        let mut new = text.clone();
        if !new.ends_with('\n') {
            new.push('\n');
        }
        new.push_str("\n  # Installed by cs320sim register-cmsis-adapter.\n");
        new.push_str(&entry_items());
        actions.push(Action { path: c.clone(), new_content: new.into_bytes(), exists: true });
    }
    let tpath = ext.join("templates/debug").join(TEMPLATE_FILE);
    match std::fs::read_to_string(&tpath) {
        Ok(t) if t == template => println!("template up to date: {}", tpath.display()),
        Ok(_) => return Err(format!("{} exists with different content; remove it first", tpath.display())),
        Err(_) => actions.push(Action { path: tpath, new_content: template.into_bytes(), exists: false }),
    }

    if actions.is_empty() {
        println!("nothing to do");
        return Ok(());
    }
    for a in &actions {
        println!("{} {}", if a.exists { "append entry to" } else { "create" }, a.path.display());
    }
    if !apply {
        println!("dry run: add --apply --backup-dir DIR to make these changes");
        return Ok(());
    }
    let backup = backup.ok_or("--apply needs --backup-dir DIR")?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let dir = backup.join(format!("cs320sim-{stamp}"));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut manifest = String::from("# original path -> backup file (empty: file was created)\n");
    for (n, a) in actions.iter().enumerate() {
        if a.exists {
            let b = dir.join(format!("{n}-{}", a.path.file_name().unwrap().to_string_lossy()));
            std::fs::copy(&a.path, &b).map_err(|e| format!("backup of {}: {e}", a.path.display()))?;
            manifest += &format!("{} -> {}\n", a.path.display(), b.display());
        } else {
            manifest += &format!("{} -> \n", a.path.display());
        }
    }
    std::fs::write(dir.join("manifest.txt"), manifest).map_err(|e| e.to_string())?;
    for a in &actions {
        std::fs::write(&a.path, &a.new_content).map_err(|e| format!("{}: {e}", a.path.display()))?;
    }
    println!("registered; backups in {}. Reload the VS Code window to use the adapter.", dir.display());
    Ok(())
}
