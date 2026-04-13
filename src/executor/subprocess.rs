use std::process::Command;

use super::{Executor, Invoker, InvokerError};

pub struct SubprocessInvoker;

impl Invoker for SubprocessInvoker {
    fn invoke(&self, command: &str, _tx: &mut dyn Executor) -> Result<(), InvokerError> {
        let status = Command::new("sh")
            .arg("-c")
            .arg(command)
            .status()
            .map_err(|e| InvokerError::Subprocess(format!("failed to spawn: {e}")))?;
        if status.success() {
            Ok(())
        } else {
            Err(InvokerError::Subprocess(format!("command exited with {status}")))
        }
    }
}
