//! Action « session Claude non interactive » (`claude -p`).

use std::path::Path;
use std::sync::atomic::AtomicBool;

use crate::exec::{Context, Outcome};
use crate::task::Task;

pub fn run_claude(_task_id: &str, _task: &Task, _ctx: &Context, _journal: &Path, _stop: &AtomicBool, _data_dir: &Path, _run_id: &str) -> Outcome {
    Outcome::impossible("Les sessions Claude ne sont pas encore prises en charge.")
}
