//! AUDIT and RECOVER (File ▸ Drawing Utilities): check a drawing for errors and fix them, and open
//! a damaged file with the fixes applied. The checks themselves live in [`crate::audit`].

use std::sync::Arc;

use serde_json::{Value, json};

use super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("audit", "Audit", run_audit)
            .menu(&["File", "Drawing Utilities", "Audit"])
            .params("{fix?: bool} → {errors, fixed, issues: [{kind, location, handle?, message, fix}], message}")
            .interactive(|_| Ok(Box::new(Audit))),
        CommandSpec::new("recover", "Recover...", run_recover)
            .menu(&["File", "Drawing Utilities", "Recover..."])
            .params("{path} | {data: base64, name} → {index, errors, fixed, issues, message}")
            .enabled(always)
            .noundo()
            .interactive(|_| Ok(Box::new(Recover))),
    ]
}

/// Check the current drawing; with `fix`, repair it (one undo step).
fn run_audit(s: &mut Session, p: &Value) -> Result<Value> {
    let fix = bool_or(p, "fix", false);
    let mut copy = (*s.doc()?).clone();
    let issues = crate::audit::repair(&mut copy);
    // A drawing without errors stays the same object: no undo step, no unsaved changes.
    if fix && !issues.is_empty() {
        s.state_mut()?.doc = Arc::new(copy);
    }
    Ok(crate::audit::Report { issues, fixed: fix }.to_json())
}

/// Open a drawing file as tolerantly as the readers allow, then fix what AUDIT finds. The
/// drawing keeps its file name; when anything was fixed it opens with unsaved changes.
fn run_recover(s: &mut Session, p: &Value) -> Result<Value> {
    let (bytes, name, path) = match (str_param(p, "path"), str_param(p, "data")) {
        (Some(path), _) => (read_path("recover", path)?, path.to_string(), Some(path.to_string())),
        (None, Some(data)) => {
            let bytes = file::base64_decode(data).ok_or_else(|| bad("recover", "invalid base64"))?;
            (bytes, str_param(p, "name").unwrap_or("Drawing.dxf").to_string(), None)
        }
        (None, None) => return Err(bad("recover", "`path` or `data` (base64) is required")),
    };
    let hooks = file::io().ok_or_else(|| bad("recover", "file formats are not available in this build"))?;
    let d = (hooks.read)(&bytes, &name).map_err(|e| bad("recover", format!("{name} can't be read: {e}")))?;
    let title = std::path::Path::new(&name).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| name.clone());
    let i = s.open_drawing(d, &title, path);
    let st = s.state_mut()?;
    let report = crate::audit::fix(Arc::make_mut(&mut st.doc));
    if report.issues.is_empty() {
        // Nothing to fix: the drawing opens unchanged, like OPEN.
        st.doc = st.saved.clone();
    }
    let mut r = report.to_json();
    r["index"] = json!(i);
    Ok(r)
}

#[cfg(not(target_arch = "wasm32"))]
fn read_path(cmd: &str, path: &str) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| bad(cmd, format!("{path}: {e}")))
}

#[cfg(target_arch = "wasm32")]
fn read_path(cmd: &str, _path: &str) -> Result<Vec<u8>> {
    Err(bad(cmd, "paths are not available on the web; pass `data`"))
}

const YES_NO: Accept = Accept { point: false, number: false, text: false, select: false, enter: true };

fn echo_result(s: &mut Session, r: &Value) {
    if let Some(m) = r.get("message").and_then(Value::as_str) {
        for l in m.lines() {
            s.echo(l.to_string());
        }
    }
}

/// AUDIT at the command line: "Fix any errors detected? [Yes/No] <N>:".
struct Audit;

impl Interactive for Audit {
    fn name(&self) -> &'static str {
        "AUDIT"
    }

    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new("Fix any errors detected?", YES_NO).kw(&["Yes", "No"]).default("N")
    }

    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let fix = match i {
            Input::Cancel => return Ok(Step::Cancel),
            Input::Enter => false,
            Input::Keyword(k) | Input::Text(k) => match self.prompt(s).match_keyword(&k).as_deref() {
                Some("Yes") => true,
                Some(_) => false,
                None => return Err(bad("audit", "answer Yes or No")),
            },
            _ => return Ok(Step::Continue),
        };
        let r = run_audit(s, &json!({ "fix": fix }))?;
        echo_result(s, &r);
        Ok(Step::Done)
    }
}

/// RECOVER at the command line (no file dialog): the file name is typed.
struct Recover;

impl Interactive for Recover {
    fn name(&self) -> &'static str {
        "RECOVER"
    }

    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new("Enter name of drawing file to recover", Accept::TEXT)
    }

    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match i {
            Input::Cancel | Input::Enter => Ok(Step::Cancel),
            Input::Text(t) | Input::Keyword(t) => {
                let path = t.trim().trim_matches('"');
                if path.is_empty() {
                    return Ok(Step::Cancel);
                }
                let r = run_recover(s, &json!({ "path": path }))?;
                echo_result(s, &r);
                Ok(Step::Done)
            }
            _ => Ok(Step::Continue),
        }
    }
}
